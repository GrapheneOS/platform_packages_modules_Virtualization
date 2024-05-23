// Copyright 2023, The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Validate device assignment written in crosvm DT with VM DTBO, and apply it
//! to platform DT.
//! Declared in separated libs for adding unit tests, which requires libstd.

#[cfg(test)]
extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::ffi::CString;
use alloc::fmt;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::CStr;
use core::iter::Iterator;
use core::mem;
use core::ops::Range;
use libfdt::{Fdt, FdtError, FdtNode, FdtNodeMut, Phandle, Reg};
use log::error;
// TODO(b/308694211): Use vmbase::hyp::{DeviceAssigningHypervisor, Error} proper for tests.
#[cfg(not(test))]
use vmbase::hyp::DeviceAssigningHypervisor;
use zerocopy::byteorder::big_endian::U32;
use zerocopy::FromBytes as _;

// TODO(b/308694211): Use cstr! from vmbase instead.
macro_rules! cstr {
    ($str:literal) => {{
        const S: &str = concat!($str, "\0");
        const C: &::core::ffi::CStr = match ::core::ffi::CStr::from_bytes_with_nul(S.as_bytes()) {
            Ok(v) => v,
            Err(_) => panic!("string contains interior NUL"),
        };
        C
    }};
}

// TODO(b/277993056): Keep constants derived from platform.dts in one place.
const CELLS_PER_INTERRUPT: usize = 3; // from /intc node in platform.dts

/// Errors in device assignment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceAssignmentError {
    /// Invalid VM DTBO
    InvalidDtbo,
    /// Invalid __symbols__
    InvalidSymbols,
    /// Malformed <reg>. Can't parse.
    MalformedReg,
    /// Invalid physical <reg> of assigned device.
    InvalidPhysReg(u64, u64),
    /// Invalid virtual <reg> of assigned device.
    InvalidReg(u64, u64),
    /// Invalid <interrupts>
    InvalidInterrupts,
    /// Malformed <iommus>
    MalformedIommus,
    /// Invalid <iommus>
    InvalidIommus,
    /// Invalid phys IOMMU node
    InvalidPhysIommu,
    /// Invalid pvIOMMU node
    InvalidPvIommu,
    /// Too many pvIOMMU
    TooManyPvIommu,
    /// Duplicated phys IOMMU IDs exist
    DuplicatedIommuIds,
    /// Duplicated pvIOMMU IDs exist
    DuplicatedPvIommuIds,
    /// Unsupported path format. Only supports full path.
    UnsupportedPathFormat,
    /// Unsupported overlay target syntax. Only supports <target-path> with full path.
    UnsupportedOverlayTarget,
    /// Unsupported PhysIommu,
    UnsupportedPhysIommu,
    /// Unsupported (pvIOMMU id, vSID) duplication. Currently the pair should be unique.
    UnsupportedPvIommusDuplication,
    /// Unsupported (IOMMU token, SID) duplication. Currently the pair should be unique.
    UnsupportedIommusDuplication,
    /// Internal error
    Internal,
    /// Unexpected error from libfdt
    UnexpectedFdtError(FdtError),
}

impl From<FdtError> for DeviceAssignmentError {
    fn from(e: FdtError) -> Self {
        DeviceAssignmentError::UnexpectedFdtError(e)
    }
}

impl fmt::Display for DeviceAssignmentError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::InvalidDtbo => write!(f, "Invalid DTBO"),
            Self::InvalidSymbols => write!(
                f,
                "Invalid property in /__symbols__. Must point to valid assignable device node."
            ),
            Self::MalformedReg => write!(f, "Malformed <reg>. Can't parse"),
            Self::InvalidReg(addr, size) => {
                write!(f, "Invalid guest MMIO region (addr: {addr:#x}, size: {size:#x})")
            }
            Self::InvalidPhysReg(addr, size) => {
                write!(f, "Invalid physical MMIO region (addr: {addr:#x}, size: {size:#x})")
            }
            Self::InvalidInterrupts => write!(f, "Invalid <interrupts>"),
            Self::MalformedIommus => write!(f, "Malformed <iommus>. Can't parse."),
            Self::InvalidIommus => {
                write!(f, "Invalid <iommus>. Failed to validate with hypervisor")
            }
            Self::InvalidPhysIommu => write!(f, "Invalid phys IOMMU node"),
            Self::InvalidPvIommu => write!(f, "Invalid pvIOMMU node"),
            Self::TooManyPvIommu => write!(
                f,
                "Too many pvIOMMU node. Insufficient pre-populated pvIOMMUs in platform DT"
            ),
            Self::DuplicatedIommuIds => {
                write!(f, "Duplicated IOMMU IDs exist. IDs must unique among iommu node")
            }
            Self::DuplicatedPvIommuIds => {
                write!(f, "Duplicated pvIOMMU IDs exist. IDs must unique among iommu node")
            }
            Self::UnsupportedPathFormat => {
                write!(f, "Unsupported UnsupportedPathFormat. Only supports full path")
            }
            Self::UnsupportedOverlayTarget => {
                write!(f, "Unsupported overlay target. Only supports 'target-path = \"/\"'")
            }
            Self::UnsupportedPhysIommu => {
                write!(f, "Unsupported Phys IOMMU. Currently only supports #iommu-cells = <1>")
            }
            Self::UnsupportedPvIommusDuplication => {
                write!(f, "Unsupported (pvIOMMU id, vSID) duplication. Currently the pair should be unique.")
            }
            Self::UnsupportedIommusDuplication => {
                write!(f, "Unsupported (IOMMU token, SID) duplication. Currently the pair should be unique.")
            }
            Self::Internal => write!(f, "Internal error"),
            Self::UnexpectedFdtError(e) => write!(f, "Unexpected Error from libfdt: {e}"),
        }
    }
}

pub type Result<T> = core::result::Result<T, DeviceAssignmentError>;

#[derive(Clone, Default, Ord, PartialOrd, Eq, PartialEq)]
pub struct DtPathTokens<'a> {
    tokens: Vec<&'a [u8]>,
}

impl<'a> fmt::Debug for DtPathTokens<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut list = f.debug_list();
        for token in &self.tokens {
            let mut bytes = token.to_vec();
            bytes.push(b'\0');
            match CString::from_vec_with_nul(bytes) {
                Ok(string) => list.entry(&string),
                Err(_) => list.entry(token),
            };
        }
        list.finish()
    }
}

impl<'a> DtPathTokens<'a> {
    fn new(path: &'a CStr) -> Result<Self> {
        if path.to_bytes().first() != Some(&b'/') {
            return Err(DeviceAssignmentError::UnsupportedPathFormat);
        }
        let tokens: Vec<_> = path
            .to_bytes()
            .split(|char| *char == b'/')
            .filter(|&component| !component.is_empty())
            .collect();
        Ok(Self { tokens })
    }

    fn to_overlay_target_path(&self) -> Result<Self> {
        if !self.is_overlayable_node() {
            return Err(DeviceAssignmentError::InvalidDtbo);
        }
        Ok(Self { tokens: self.tokens.as_slice()[2..].to_vec() })
    }

    fn to_cstring(&self) -> CString {
        if self.tokens.is_empty() {
            return CString::new(*b"/\0").unwrap();
        }

        let size = self.tokens.iter().fold(0, |sum, token| sum + token.len() + 1);
        let mut path = Vec::with_capacity(size + 1);
        for token in &self.tokens {
            path.push(b'/');
            path.extend_from_slice(token);
        }
        path.push(b'\0');

        CString::from_vec_with_nul(path).unwrap()
    }

    fn is_overlayable_node(&self) -> bool {
        self.tokens.get(1) == Some(&&b"__overlay__"[..])
    }
}

#[derive(Debug, Eq, PartialEq)]
enum DeviceTreeChildrenMask {
    Partial(Vec<DeviceTreeMask>),
    All,
}

#[derive(Eq, PartialEq)]
struct DeviceTreeMask {
    name_bytes: Vec<u8>,
    children: DeviceTreeChildrenMask,
}

impl fmt::Debug for DeviceTreeMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name_bytes = [self.name_bytes.as_slice(), b"\0"].concat();

        f.debug_struct("DeviceTreeMask")
            .field("name", &CStr::from_bytes_with_nul(&name_bytes).unwrap())
            .field("children", &self.children)
            .finish()
    }
}

impl DeviceTreeMask {
    fn new() -> Self {
        Self { name_bytes: b"/".to_vec(), children: DeviceTreeChildrenMask::Partial(Vec::new()) }
    }

    fn mask_internal(&mut self, path: &DtPathTokens, leaf_mask: DeviceTreeChildrenMask) -> bool {
        let mut iter = self;
        let mut newly_masked = false;
        'next_token: for path_token in &path.tokens {
            let DeviceTreeChildrenMask::Partial(ref mut children) = &mut iter.children else {
                return false;
            };

            // Note: Can't use iterator for 'get or insert'. (a.k.a. polonius Rust)
            #[allow(clippy::needless_range_loop)]
            for i in 0..children.len() {
                if children[i].name_bytes.as_slice() == *path_token {
                    iter = &mut children[i];
                    newly_masked = false;
                    continue 'next_token;
                }
            }
            let child = Self {
                name_bytes: path_token.to_vec(),
                children: DeviceTreeChildrenMask::Partial(Vec::new()),
            };
            children.push(child);
            newly_masked = true;
            iter = children.last_mut().unwrap()
        }
        iter.children = leaf_mask;
        newly_masked
    }

    fn mask(&mut self, path: &DtPathTokens) -> bool {
        self.mask_internal(path, DeviceTreeChildrenMask::Partial(Vec::new()))
    }

    fn mask_all(&mut self, path: &DtPathTokens) {
        self.mask_internal(path, DeviceTreeChildrenMask::All);
    }
}

/// Represents VM DTBO
#[repr(transparent)]
pub struct VmDtbo(Fdt);

impl VmDtbo {
    /// Wraps a mutable slice containing a VM DTBO.
    ///
    /// Fails if the VM DTBO does not pass validation.
    pub fn from_mut_slice(dtbo: &mut [u8]) -> Result<&mut Self> {
        // This validates DTBO
        let fdt = Fdt::from_mut_slice(dtbo)?;
        // SAFETY: VmDtbo is a transparent wrapper around Fdt, so representation is the same.
        Ok(unsafe { mem::transmute::<&mut Fdt, &mut Self>(fdt) })
    }

    // Locates device node path as if the given dtbo node path is assigned and VM DTBO is overlaid.
    // For given dtbo node path, this concatenates <target-path> of the enclosing fragment and
    // relative path from __overlay__ node.
    //
    // Here's an example with sample VM DTBO:
    //    / {
    //       fragment@rng {
    //         target-path = "/";  // Always 'target-path = "/"'. Disallows <target> or other path.
    //         __overlay__ {
    //           rng { ... };      // Actual device node is here. If overlaid, path would be "/rng"
    //         };
    //       };
    //       __symbols__ {         // Contains list of assignable devices
    //         rng = "/fragment@rng/__overlay__/rng";
    //       };
    //    };
    //
    // Then locate_overlay_target_path(cstr!("/fragment@rng/__overlay__/rng")) is Ok("/rng")
    //
    // Contrary to fdt_overlay_target_offset(), this API enforces overlay target property
    // 'target-path = "/"', so the overlay doesn't modify and/or append platform DT's existing
    // node and/or properties. The enforcement is for compatibility reason.
    fn locate_overlay_target_path(
        &self,
        dtbo_node_path: &DtPathTokens,
        dtbo_node: &FdtNode,
    ) -> Result<CString> {
        let fragment_node = dtbo_node.supernode_at_depth(1)?;
        let target_path = fragment_node
            .getprop_str(cstr!("target-path"))?
            .ok_or(DeviceAssignmentError::InvalidDtbo)?;
        if target_path != cstr!("/") {
            return Err(DeviceAssignmentError::UnsupportedOverlayTarget);
        }

        let overlaid_path = dtbo_node_path.to_overlay_target_path()?;
        Ok(overlaid_path.to_cstring())
    }

    fn parse_physical_iommus(physical_node: &FdtNode) -> Result<BTreeMap<Phandle, PhysIommu>> {
        let mut phys_iommus = BTreeMap::new();
        for (node, _) in physical_node.descendants() {
            let Some(phandle) = node.get_phandle()? else {
                continue; // Skips unreachable IOMMU node
            };
            let Some(iommu) = PhysIommu::parse(&node)? else {
                continue; // Skip if not a PhysIommu.
            };
            if phys_iommus.insert(phandle, iommu).is_some() {
                return Err(FdtError::BadPhandle.into());
            }
        }
        Self::validate_physical_iommus(&phys_iommus)?;
        Ok(phys_iommus)
    }

    fn validate_physical_iommus(phys_iommus: &BTreeMap<Phandle, PhysIommu>) -> Result<()> {
        let unique_iommus: BTreeSet<_> = phys_iommus.values().cloned().collect();
        if phys_iommus.len() != unique_iommus.len() {
            return Err(DeviceAssignmentError::DuplicatedIommuIds);
        }
        Ok(())
    }

    fn validate_physical_devices(
        physical_devices: &BTreeMap<Phandle, PhysicalDeviceInfo>,
    ) -> Result<()> {
        // Only need to validate iommus because <reg> will be validated together with PV <reg>
        // see: DeviceAssignmentInfo::validate_all_regs().
        let mut all_iommus = BTreeSet::new();
        for physical_device in physical_devices.values() {
            for iommu in &physical_device.iommus {
                if !all_iommus.insert(iommu) {
                    error!("Unsupported phys IOMMU duplication found, <iommus> = {iommu:?}");
                    return Err(DeviceAssignmentError::UnsupportedIommusDuplication);
                }
            }
        }
        Ok(())
    }

    fn parse_physical_devices_with_iommus(
        physical_node: &FdtNode,
        phys_iommus: &BTreeMap<Phandle, PhysIommu>,
    ) -> Result<BTreeMap<Phandle, PhysicalDeviceInfo>> {
        let mut physical_devices = BTreeMap::new();
        for (node, _) in physical_node.descendants() {
            let Some(info) = PhysicalDeviceInfo::parse(&node, phys_iommus)? else {
                continue;
            };
            if physical_devices.insert(info.target, info).is_some() {
                return Err(DeviceAssignmentError::InvalidDtbo);
            }
        }
        Self::validate_physical_devices(&physical_devices)?;
        Ok(physical_devices)
    }

    /// Parses Physical devices in VM DTBO
    fn parse_physical_devices(&self) -> Result<BTreeMap<Phandle, PhysicalDeviceInfo>> {
        let Some(physical_node) = self.as_ref().node(cstr!("/host"))? else {
            return Ok(BTreeMap::new());
        };

        let phys_iommus = Self::parse_physical_iommus(&physical_node)?;
        Self::parse_physical_devices_with_iommus(&physical_node, &phys_iommus)
    }

    fn node(&self, path: &DtPathTokens) -> Result<Option<FdtNode>> {
        let mut node = self.as_ref().root();
        for token in &path.tokens {
            let Some(subnode) = node.subnode_with_name_bytes(token)? else {
                return Ok(None);
            };
            node = subnode;
        }
        Ok(Some(node))
    }

    fn collect_overlayable_nodes_with_phandle(&self) -> Result<BTreeMap<Phandle, DtPathTokens>> {
        let mut paths = BTreeMap::new();
        let mut path: DtPathTokens = Default::default();
        let root = self.as_ref().root();
        for (node, depth) in root.descendants() {
            path.tokens.truncate(depth - 1);
            path.tokens.push(node.name()?.to_bytes());
            if !path.is_overlayable_node() {
                continue;
            }
            if let Some(phandle) = node.get_phandle()? {
                paths.insert(phandle, path.clone());
            }
        }
        Ok(paths)
    }

    fn collect_phandle_references_from_overlayable_nodes(
        &self,
    ) -> Result<BTreeMap<DtPathTokens, Vec<Phandle>>> {
        const CELL_SIZE: usize = core::mem::size_of::<u32>();

        let vm_dtbo = self.as_ref();

        let mut phandle_map = BTreeMap::new();
        let Some(local_fixups) = vm_dtbo.node(cstr!("/__local_fixups__"))? else {
            return Ok(phandle_map);
        };

        let mut path: DtPathTokens = Default::default();
        for (fixup_node, depth) in local_fixups.descendants() {
            let node_name = fixup_node.name()?;
            path.tokens.truncate(depth - 1);
            path.tokens.push(node_name.to_bytes());
            if path.tokens.len() != depth {
                return Err(DeviceAssignmentError::Internal);
            }
            if !path.is_overlayable_node() {
                continue;
            }
            let target_node = self.node(&path)?.ok_or(DeviceAssignmentError::InvalidDtbo)?;

            let mut phandles = vec![];
            for fixup_prop in fixup_node.properties()? {
                let target_prop = target_node
                    .getprop(fixup_prop.name()?)
                    .or(Err(DeviceAssignmentError::InvalidDtbo))?
                    .ok_or(DeviceAssignmentError::InvalidDtbo)?;
                let fixup_prop_values = fixup_prop.value()?;
                if fixup_prop_values.is_empty() || fixup_prop_values.len() % CELL_SIZE != 0 {
                    return Err(DeviceAssignmentError::InvalidDtbo);
                }

                for fixup_prop_cell in fixup_prop_values.chunks(CELL_SIZE) {
                    let phandle_offset: usize = u32::from_be_bytes(
                        fixup_prop_cell.try_into().or(Err(DeviceAssignmentError::InvalidDtbo))?,
                    )
                    .try_into()
                    .or(Err(DeviceAssignmentError::InvalidDtbo))?;
                    if phandle_offset % CELL_SIZE != 0 {
                        return Err(DeviceAssignmentError::InvalidDtbo);
                    }
                    let phandle_value = target_prop
                        .get(phandle_offset..phandle_offset + CELL_SIZE)
                        .ok_or(DeviceAssignmentError::InvalidDtbo)?;
                    let phandle: Phandle = U32::ref_from(phandle_value)
                        .unwrap()
                        .get()
                        .try_into()
                        .or(Err(DeviceAssignmentError::InvalidDtbo))?;

                    phandles.push(phandle);
                }
            }
            if !phandles.is_empty() {
                phandle_map.insert(path.clone(), phandles);
            }
        }

        Ok(phandle_map)
    }

    fn build_mask(&self, assigned_devices: Vec<DtPathTokens>) -> Result<DeviceTreeMask> {
        if assigned_devices.is_empty() {
            return Err(DeviceAssignmentError::Internal);
        }

        let dependencies = self.collect_phandle_references_from_overlayable_nodes()?;
        let paths = self.collect_overlayable_nodes_with_phandle()?;

        let mut mask = DeviceTreeMask::new();
        let mut stack = assigned_devices;
        while let Some(path) = stack.pop() {
            if !mask.mask(&path) {
                continue;
            }
            let Some(dst_phandles) = dependencies.get(&path) else {
                continue;
            };
            for dst_phandle in dst_phandles {
                let dst_path = paths.get(dst_phandle).ok_or(DeviceAssignmentError::Internal)?;
                stack.push(dst_path.clone());
            }
        }

        Ok(mask)
    }
}

fn filter_dangling_symbols(fdt: &mut Fdt) -> Result<()> {
    if let Some(symbols) = fdt.symbols()? {
        let mut removed = vec![];
        for prop in symbols.properties()? {
            let path = CStr::from_bytes_with_nul(prop.value()?)
                .map_err(|_| DeviceAssignmentError::Internal)?;
            if fdt.node(path)?.is_none() {
                let name = prop.name()?;
                removed.push(CString::from(name));
            }
        }

        let mut symbols = fdt.symbols_mut()?.unwrap();
        for name in removed {
            symbols.nop_property(&name)?;
        }
    }
    Ok(())
}

impl AsRef<Fdt> for VmDtbo {
    fn as_ref(&self) -> &Fdt {
        &self.0
    }
}

impl AsMut<Fdt> for VmDtbo {
    fn as_mut(&mut self) -> &mut Fdt {
        &mut self.0
    }
}

// Filter any node that isn't masked by DeviceTreeMask.
fn filter_with_mask(anchor: FdtNodeMut, mask: &DeviceTreeMask) -> Result<()> {
    let mut stack = vec![mask];
    let mut iter = anchor.next_node(0)?;
    while let Some((node, depth)) = iter {
        stack.truncate(depth);
        let parent_mask = stack.last().unwrap();
        let DeviceTreeChildrenMask::Partial(parent_mask_children) = &parent_mask.children else {
            // Shouldn't happen. We only step-in if parent has DeviceTreeChildrenMask::Partial.
            return Err(DeviceAssignmentError::Internal);
        };

        let name = node.as_node().name()?.to_bytes();
        let mask = parent_mask_children.iter().find(|child_mask| child_mask.name_bytes == name);
        if let Some(masked) = mask {
            if let DeviceTreeChildrenMask::Partial(_) = &masked.children {
                // This node is partially masked. Stepping-in.
                stack.push(masked);
                iter = node.next_node(depth)?;
            } else {
                // This node is fully masked. Stepping-out.
                iter = node.next_node_skip_subnodes(depth)?;
            }
        } else {
            // This node isn't masked.
            iter = node.delete_and_next_node(depth)?;
        }
    }

    Ok(())
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct PvIommu {
    // ID from pvIOMMU node
    id: u32,
}

impl PvIommu {
    fn parse(node: &FdtNode) -> Result<Self> {
        let iommu_cells = node
            .getprop_u32(cstr!("#iommu-cells"))?
            .ok_or(DeviceAssignmentError::InvalidPvIommu)?;
        // Ensures #iommu-cells = <1>. It means that `<iommus>` entry contains pair of
        // (pvIOMMU ID, vSID)
        if iommu_cells != 1 {
            return Err(DeviceAssignmentError::InvalidPvIommu);
        }
        let id = node.getprop_u32(cstr!("id"))?.ok_or(DeviceAssignmentError::InvalidPvIommu)?;
        Ok(Self { id })
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct Vsid(u32);

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct Sid(u64);

impl From<u32> for Sid {
    fn from(sid: u32) -> Self {
        Self(sid.into())
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct DeviceReg {
    addr: u64,
    size: u64,
}

impl DeviceReg {
    pub fn overlaps(&self, range: &Range<u64>) -> bool {
        self.addr < range.end && range.start < self.addr.checked_add(self.size).unwrap()
    }
}

impl TryFrom<Reg<u64>> for DeviceReg {
    type Error = DeviceAssignmentError;

    fn try_from(reg: Reg<u64>) -> Result<Self> {
        Ok(Self { addr: reg.addr, size: reg.size.ok_or(DeviceAssignmentError::MalformedReg)? })
    }
}

fn parse_node_reg(node: &FdtNode) -> Result<Vec<DeviceReg>> {
    node.reg()?
        .ok_or(DeviceAssignmentError::MalformedReg)?
        .map(DeviceReg::try_from)
        .collect::<Result<Vec<_>>>()
}

fn to_be_bytes(reg: &[DeviceReg]) -> Vec<u8> {
    let mut reg_cells = vec![];
    for x in reg {
        reg_cells.extend_from_slice(&x.addr.to_be_bytes());
        reg_cells.extend_from_slice(&x.size.to_be_bytes());
    }
    reg_cells
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct PhysIommu {
    token: u64,
}

impl PhysIommu {
    fn parse(node: &FdtNode) -> Result<Option<Self>> {
        let Some(token) = node.getprop_u64(cstr!("android,pvmfw,token"))? else {
            return Ok(None);
        };
        let Some(iommu_cells) = node.getprop_u32(cstr!("#iommu-cells"))? else {
            return Err(DeviceAssignmentError::InvalidPhysIommu);
        };
        // Currently only supports #iommu-cells = <1>.
        // In that case `<iommus>` entry contains pair of (pIOMMU phandle, Sid token)
        if iommu_cells != 1 {
            return Err(DeviceAssignmentError::UnsupportedPhysIommu);
        }
        Ok(Some(Self { token }))
    }
}

#[derive(Debug)]
struct PhysicalDeviceInfo {
    target: Phandle,
    reg: Vec<DeviceReg>,
    iommus: Vec<(PhysIommu, Sid)>,
}

impl PhysicalDeviceInfo {
    fn parse_iommus(
        node: &FdtNode,
        phys_iommus: &BTreeMap<Phandle, PhysIommu>,
    ) -> Result<Vec<(PhysIommu, Sid)>> {
        let mut iommus = vec![];
        let Some(mut cells) = node.getprop_cells(cstr!("iommus"))? else {
            return Ok(iommus);
        };
        while let Some(cell) = cells.next() {
            // Parse pIOMMU ID
            let phandle =
                Phandle::try_from(cell).or(Err(DeviceAssignmentError::MalformedIommus))?;
            let iommu = phys_iommus.get(&phandle).ok_or(DeviceAssignmentError::MalformedIommus)?;

            // Parse Sid
            let Some(cell) = cells.next() else {
                return Err(DeviceAssignmentError::MalformedIommus);
            };

            iommus.push((*iommu, Sid::from(cell)));
        }
        Ok(iommus)
    }

    fn parse(node: &FdtNode, phys_iommus: &BTreeMap<Phandle, PhysIommu>) -> Result<Option<Self>> {
        let Some(phandle) = node.getprop_u32(cstr!("android,pvmfw,target"))? else {
            return Ok(None);
        };
        let target = Phandle::try_from(phandle)?;
        let reg = parse_node_reg(node)?;
        let iommus = Self::parse_iommus(node, phys_iommus)?;
        Ok(Some(Self { target, reg, iommus }))
    }
}

/// Assigned device information parsed from crosvm DT.
/// Keeps everything in the owned data because underlying FDT will be reused for platform DT.
#[derive(Debug, Eq, PartialEq)]
struct AssignedDeviceInfo {
    // Node path of assigned device (e.g. "/rng")
    node_path: CString,
    // <reg> property from the crosvm DT
    reg: Vec<DeviceReg>,
    // <interrupts> property from the crosvm DT
    interrupts: Vec<u8>,
    // Parsed <iommus> property from the crosvm DT. Tuple of PvIommu and vSID.
    iommus: Vec<(PvIommu, Vsid)>,
}

impl AssignedDeviceInfo {
    fn validate_reg(
        device_reg: &[DeviceReg],
        physical_device_reg: &[DeviceReg],
        hypervisor: &dyn DeviceAssigningHypervisor,
    ) -> Result<()> {
        let mut virt_regs = device_reg.iter();
        let mut phys_regs = physical_device_reg.iter();
        // TODO(b/308694211): Move this constant to vmbase::layout once vmbase is std-compatible.
        const PVMFW_RANGE: Range<u64> = 0x7fc0_0000..0x8000_0000;
        // PV reg and physical reg should have 1:1 match in order.
        for (reg, phys_reg) in virt_regs.by_ref().zip(phys_regs.by_ref()) {
            if reg.overlaps(&PVMFW_RANGE) {
                return Err(DeviceAssignmentError::InvalidReg(reg.addr, reg.size));
            }
            // If this call returns successfully, hyp has mapped the MMIO region at `reg`.
            let addr = hypervisor.get_phys_mmio_token(reg.addr, reg.size).map_err(|e| {
                error!("Hypervisor error while requesting MMIO token: {e}");
                DeviceAssignmentError::InvalidReg(reg.addr, reg.size)
            })?;
            // Only check address because hypervisor guarantees size match when success.
            if phys_reg.addr != addr {
                error!("Assigned device {reg:x?} has unexpected physical address");
                return Err(DeviceAssignmentError::InvalidPhysReg(addr, reg.size));
            }
        }

        if let Some(DeviceReg { addr, size }) = virt_regs.next() {
            return Err(DeviceAssignmentError::InvalidReg(*addr, *size));
        }

        if let Some(DeviceReg { addr, size }) = phys_regs.next() {
            return Err(DeviceAssignmentError::InvalidPhysReg(*addr, *size));
        }

        Ok(())
    }

    fn parse_interrupts(node: &FdtNode) -> Result<Vec<u8>> {
        // Validation: Validate if interrupts cell numbers are multiple of #interrupt-cells.
        // We can't know how many interrupts would exist.
        let interrupts_cells = node
            .getprop_cells(cstr!("interrupts"))?
            .ok_or(DeviceAssignmentError::InvalidInterrupts)?
            .count();
        if interrupts_cells % CELLS_PER_INTERRUPT != 0 {
            return Err(DeviceAssignmentError::InvalidInterrupts);
        }

        // Once validated, keep the raw bytes so patch can be done with setprop()
        Ok(node.getprop(cstr!("interrupts")).unwrap().unwrap().into())
    }

    // TODO(b/277993056): Also validate /__local_fixups__ to ensure that <iommus> has phandle.
    fn parse_iommus(
        node: &FdtNode,
        pviommus: &BTreeMap<Phandle, PvIommu>,
    ) -> Result<Vec<(PvIommu, Vsid)>> {
        let mut iommus = vec![];
        let Some(mut cells) = node.getprop_cells(cstr!("iommus"))? else {
            return Ok(iommus);
        };
        while let Some(cell) = cells.next() {
            // Parse pvIOMMU ID
            let phandle =
                Phandle::try_from(cell).or(Err(DeviceAssignmentError::MalformedIommus))?;
            let pviommu = pviommus.get(&phandle).ok_or(DeviceAssignmentError::MalformedIommus)?;

            // Parse vSID
            let Some(cell) = cells.next() else {
                return Err(DeviceAssignmentError::MalformedIommus);
            };
            let vsid = Vsid(cell);

            iommus.push((*pviommu, vsid));
        }
        Ok(iommus)
    }

    fn validate_iommus(
        iommus: &[(PvIommu, Vsid)],
        physical_device_iommu: &[(PhysIommu, Sid)],
        hypervisor: &dyn DeviceAssigningHypervisor,
    ) -> Result<()> {
        if iommus.len() != physical_device_iommu.len() {
            return Err(DeviceAssignmentError::InvalidIommus);
        }
        // pvIOMMU can be reordered, and hypervisor may not guarantee 1:1 mapping.
        // So we need to mark what's matched or not.
        let mut physical_device_iommu = physical_device_iommu.to_vec();
        for (pviommu, vsid) in iommus {
            let (id, sid) =
                hypervisor.get_phys_iommu_token(pviommu.id.into(), vsid.0.into()).map_err(|e| {
                    error!("Hypervisor error while requesting IOMMU token ({pviommu:?}, {vsid:?}): {e}");
                    DeviceAssignmentError::InvalidIommus
                })?;

            let pos = physical_device_iommu
                .iter()
                .position(|(phys_iommu, phys_sid)| (phys_iommu.token, phys_sid.0) == (id, sid));
            match pos {
                Some(pos) => physical_device_iommu.remove(pos),
                None => {
                    error!("Failed to validate device <iommus>. No matching phys iommu or duplicated mapping for pviommu={pviommu:?}, vsid={vsid:?}");
                    return Err(DeviceAssignmentError::InvalidIommus);
                }
            };
        }
        Ok(())
    }

    fn parse(
        fdt: &Fdt,
        vm_dtbo: &VmDtbo,
        dtbo_node_path: &DtPathTokens,
        physical_devices: &BTreeMap<Phandle, PhysicalDeviceInfo>,
        pviommus: &BTreeMap<Phandle, PvIommu>,
        hypervisor: &dyn DeviceAssigningHypervisor,
    ) -> Result<Option<Self>> {
        let dtbo_node =
            vm_dtbo.node(dtbo_node_path)?.ok_or(DeviceAssignmentError::InvalidSymbols)?;
        let node_path = vm_dtbo.locate_overlay_target_path(dtbo_node_path, &dtbo_node)?;

        let Some(node) = fdt.node(&node_path)? else { return Ok(None) };

        // Currently can only assign devices backed by physical devices.
        let phandle = dtbo_node.get_phandle()?.ok_or(DeviceAssignmentError::InvalidDtbo)?;
        let Some(physical_device) = physical_devices.get(&phandle) else {
            // If labeled DT node isn't backed by physical device node, then just return None.
            // It's not an error because such node can be a dependency of assignable device nodes.
            return Ok(None);
        };

        let reg = parse_node_reg(&node)?;
        Self::validate_reg(&reg, &physical_device.reg, hypervisor)?;

        let interrupts = Self::parse_interrupts(&node)?;

        let iommus = Self::parse_iommus(&node, pviommus)?;
        Self::validate_iommus(&iommus, &physical_device.iommus, hypervisor)?;

        Ok(Some(Self { node_path, reg, interrupts, iommus }))
    }

    fn patch(&self, fdt: &mut Fdt, pviommu_phandles: &BTreeMap<PvIommu, Phandle>) -> Result<()> {
        let mut dst = fdt.node_mut(&self.node_path)?.unwrap();
        dst.setprop(cstr!("reg"), &to_be_bytes(&self.reg))?;
        dst.setprop(cstr!("interrupts"), &self.interrupts)?;
        let mut iommus = Vec::with_capacity(8 * self.iommus.len());
        for (pviommu, vsid) in &self.iommus {
            let phandle = pviommu_phandles.get(pviommu).unwrap();
            iommus.extend_from_slice(&u32::from(*phandle).to_be_bytes());
            iommus.extend_from_slice(&vsid.0.to_be_bytes());
        }
        dst.setprop(cstr!("iommus"), &iommus)?;

        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct DeviceAssignmentInfo {
    pviommus: BTreeSet<PvIommu>,
    assigned_devices: Vec<AssignedDeviceInfo>,
    vm_dtbo_mask: DeviceTreeMask,
}

impl DeviceAssignmentInfo {
    const PVIOMMU_COMPATIBLE: &'static CStr = cstr!("pkvm,pviommu");

    /// Parses pvIOMMUs in fdt
    // Note: This will validate pvIOMMU ids' uniqueness, even when unassigned.
    fn parse_pviommus(fdt: &Fdt) -> Result<BTreeMap<Phandle, PvIommu>> {
        let mut pviommus = BTreeMap::new();
        for compatible in fdt.compatible_nodes(Self::PVIOMMU_COMPATIBLE)? {
            let Some(phandle) = compatible.get_phandle()? else {
                continue; // Skips unreachable pvIOMMU node
            };
            let pviommu = PvIommu::parse(&compatible)?;
            if pviommus.insert(phandle, pviommu).is_some() {
                return Err(FdtError::BadPhandle.into());
            }
        }
        Ok(pviommus)
    }

    fn validate_pviommu_topology(assigned_devices: &[AssignedDeviceInfo]) -> Result<()> {
        let mut all_iommus = BTreeSet::new();
        for assigned_device in assigned_devices {
            for iommu in &assigned_device.iommus {
                if !all_iommus.insert(iommu) {
                    error!("Unsupported pvIOMMU duplication found, <iommus> = {iommu:?}");
                    return Err(DeviceAssignmentError::UnsupportedPvIommusDuplication);
                }
            }
        }
        Ok(())
    }

    // TODO(b/308694211): Remove this workaround for visibility once using
    // vmbase::hyp::DeviceAssigningHypervisor for tests.
    #[cfg(test)]
    fn parse(
        fdt: &Fdt,
        vm_dtbo: &VmDtbo,
        hypervisor: &dyn DeviceAssigningHypervisor,
    ) -> Result<Option<Self>> {
        Self::internal_parse(fdt, vm_dtbo, hypervisor)
    }

    #[cfg(not(test))]
    /// Parses fdt and vm_dtbo, and creates new DeviceAssignmentInfo
    // TODO(b/277993056): Parse __local_fixups__
    // TODO(b/277993056): Parse __fixups__
    pub fn parse(
        fdt: &Fdt,
        vm_dtbo: &VmDtbo,
        hypervisor: &dyn DeviceAssigningHypervisor,
    ) -> Result<Option<Self>> {
        Self::internal_parse(fdt, vm_dtbo, hypervisor)
    }

    fn internal_parse(
        fdt: &Fdt,
        vm_dtbo: &VmDtbo,
        hypervisor: &dyn DeviceAssigningHypervisor,
    ) -> Result<Option<Self>> {
        let Some(symbols_node) = vm_dtbo.as_ref().symbols()? else {
            // /__symbols__ should contain all assignable devices.
            // If empty, then nothing can be assigned.
            return Ok(None);
        };

        let pviommus = Self::parse_pviommus(fdt)?;
        let unique_pviommus: BTreeSet<_> = pviommus.values().cloned().collect();
        if pviommus.len() != unique_pviommus.len() {
            return Err(DeviceAssignmentError::DuplicatedPvIommuIds);
        }

        let physical_devices = vm_dtbo.parse_physical_devices()?;

        let mut assigned_devices = vec![];
        let mut assigned_device_paths = vec![];
        for symbol_prop in symbols_node.properties()? {
            let symbol_prop_value = symbol_prop.value()?;
            let dtbo_node_path = CStr::from_bytes_with_nul(symbol_prop_value)
                .or(Err(DeviceAssignmentError::InvalidSymbols))?;
            let dtbo_node_path = DtPathTokens::new(dtbo_node_path)?;
            if !dtbo_node_path.is_overlayable_node() {
                continue;
            }
            let assigned_device = AssignedDeviceInfo::parse(
                fdt,
                vm_dtbo,
                &dtbo_node_path,
                &physical_devices,
                &pviommus,
                hypervisor,
            )?;
            if let Some(assigned_device) = assigned_device {
                assigned_devices.push(assigned_device);
                assigned_device_paths.push(dtbo_node_path);
            }
        }
        if assigned_devices.is_empty() {
            return Ok(None);
        }

        Self::validate_pviommu_topology(&assigned_devices)?;

        let mut vm_dtbo_mask = vm_dtbo.build_mask(assigned_device_paths)?;
        vm_dtbo_mask.mask_all(&DtPathTokens::new(cstr!("/__local_fixups__"))?);
        vm_dtbo_mask.mask_all(&DtPathTokens::new(cstr!("/__symbols__"))?);

        // Note: Any node without __overlay__ will be ignored by fdt_apply_overlay,
        // so doesn't need to be filtered.

        Ok(Some(Self { pviommus: unique_pviommus, assigned_devices, vm_dtbo_mask }))
    }

    /// Filters VM DTBO to only contain necessary information for booting pVM
    pub fn filter(&self, vm_dtbo: &mut VmDtbo) -> Result<()> {
        let vm_dtbo = vm_dtbo.as_mut();

        // Filter unused references in /__local_fixups__
        if let Some(local_fixups) = vm_dtbo.node_mut(cstr!("/__local_fixups__"))? {
            filter_with_mask(local_fixups, &self.vm_dtbo_mask)?;
        }

        // Filter unused nodes in rest of tree
        let root = vm_dtbo.root_mut();
        filter_with_mask(root, &self.vm_dtbo_mask)?;

        filter_dangling_symbols(vm_dtbo)
    }

    fn patch_pviommus(&self, fdt: &mut Fdt) -> Result<BTreeMap<PvIommu, Phandle>> {
        let mut compatible = fdt.root_mut().next_compatible(Self::PVIOMMU_COMPATIBLE)?;
        let mut pviommu_phandles = BTreeMap::new();

        for pviommu in &self.pviommus {
            let mut node = compatible.ok_or(DeviceAssignmentError::TooManyPvIommu)?;
            let phandle = node.as_node().get_phandle()?.ok_or(DeviceAssignmentError::Internal)?;
            node.setprop_inplace(cstr!("id"), &pviommu.id.to_be_bytes())?;
            if pviommu_phandles.insert(*pviommu, phandle).is_some() {
                return Err(DeviceAssignmentError::Internal);
            }
            compatible = node.next_compatible(Self::PVIOMMU_COMPATIBLE)?;
        }

        // Filters pre-populated but unassigned pvIOMMUs.
        while let Some(filtered_pviommu) = compatible {
            compatible = filtered_pviommu.delete_and_next_compatible(Self::PVIOMMU_COMPATIBLE)?;
        }

        Ok(pviommu_phandles)
    }

    pub fn patch(&self, fdt: &mut Fdt) -> Result<()> {
        let pviommu_phandles = self.patch_pviommus(fdt)?;

        // Patches assigned devices
        for device in &self.assigned_devices {
            device.patch(fdt, &pviommu_phandles)?;
        }

        // Removes any dangling references in __symbols__ (e.g. removed pvIOMMUs)
        filter_dangling_symbols(fdt)
    }
}

/// Cleans device trees not to contain any pre-populated nodes/props for device assignment.
pub fn clean(fdt: &mut Fdt) -> Result<()> {
    let mut compatible = fdt.root_mut().next_compatible(cstr!("pkvm,pviommu"))?;
    // Filters pre-populated
    while let Some(filtered_pviommu) = compatible {
        compatible = filtered_pviommu.delete_and_next_compatible(cstr!("pkvm,pviommu"))?;
    }

    // Removes any dangling references in __symbols__ (e.g. removed pvIOMMUs)
    filter_dangling_symbols(fdt)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
enum MockHypervisorError {
    FailedGetPhysMmioToken,
    FailedGetPhysIommuToken,
}

#[cfg(test)]
type MockHypervisorResult<T> = core::result::Result<T, MockHypervisorError>;

#[cfg(test)]
impl fmt::Display for MockHypervisorError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MockHypervisorError::FailedGetPhysMmioToken => {
                write!(f, "Failed to get physical MMIO token")
            }
            MockHypervisorError::FailedGetPhysIommuToken => {
                write!(f, "Failed to get physical IOMMU token")
            }
        }
    }
}

#[cfg(test)]
trait DeviceAssigningHypervisor {
    /// Returns MMIO token.
    fn get_phys_mmio_token(&self, base_ipa: u64, size: u64) -> MockHypervisorResult<u64>;

    /// Returns DMA token as a tuple of (phys_iommu_id, phys_sid).
    fn get_phys_iommu_token(&self, pviommu_id: u64, vsid: u64) -> MockHypervisorResult<(u64, u64)>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::{BTreeMap, BTreeSet};
    use dts::Dts;
    use std::fs;
    use std::path::Path;

    const VM_DTBO_FILE_PATH: &str = "test_pvmfw_devices_vm_dtbo.dtbo";
    const VM_DTBO_WITHOUT_SYMBOLS_FILE_PATH: &str =
        "test_pvmfw_devices_vm_dtbo_without_symbols.dtbo";
    const VM_DTBO_WITH_DUPLICATED_IOMMUS_FILE_PATH: &str =
        "test_pvmfw_devices_vm_dtbo_with_duplicated_iommus.dtbo";
    const VM_DTBO_WITH_DEPENDENCIES_FILE_PATH: &str =
        "test_pvmfw_devices_vm_dtbo_with_dependencies.dtbo";
    const FDT_WITHOUT_IOMMUS_FILE_PATH: &str = "test_pvmfw_devices_without_iommus.dtb";
    const FDT_WITHOUT_DEVICE_FILE_PATH: &str = "test_pvmfw_devices_without_device.dtb";
    const FDT_FILE_PATH: &str = "test_pvmfw_devices_with_rng.dtb";
    const FDT_WITH_DEVICE_OVERLAPPING_PVMFW: &str = "test_pvmfw_devices_overlapping_pvmfw.dtb";
    const FDT_WITH_MULTIPLE_DEVICES_IOMMUS_FILE_PATH: &str =
        "test_pvmfw_devices_with_multiple_devices_iommus.dtb";
    const FDT_WITH_IOMMU_SHARING: &str = "test_pvmfw_devices_with_iommu_sharing.dtb";
    const FDT_WITH_IOMMU_ID_CONFLICT: &str = "test_pvmfw_devices_with_iommu_id_conflict.dtb";
    const FDT_WITH_DUPLICATED_PVIOMMUS_FILE_PATH: &str =
        "test_pvmfw_devices_with_duplicated_pviommus.dtb";
    const FDT_WITH_MULTIPLE_REG_IOMMU_FILE_PATH: &str =
        "test_pvmfw_devices_with_multiple_reg_iommus.dtb";
    const FDT_WITH_DEPENDENCY_FILE_PATH: &str = "test_pvmfw_devices_with_dependency.dtb";
    const FDT_WITH_MULTIPLE_DEPENDENCIES_FILE_PATH: &str =
        "test_pvmfw_devices_with_multiple_dependencies.dtb";
    const FDT_WITH_DEPENDENCY_LOOP_FILE_PATH: &str = "test_pvmfw_devices_with_dependency_loop.dtb";

    const EXPECTED_FDT_WITH_DEPENDENCY_FILE_PATH: &str = "expected_dt_with_dependency.dtb";
    const EXPECTED_FDT_WITH_MULTIPLE_DEPENDENCIES_FILE_PATH: &str =
        "expected_dt_with_multiple_dependencies.dtb";
    const EXPECTED_FDT_WITH_DEPENDENCY_LOOP_FILE_PATH: &str =
        "expected_dt_with_dependency_loop.dtb";

    #[derive(Debug, Default)]
    struct MockHypervisor {
        mmio_tokens: BTreeMap<(u64, u64), u64>,
        iommu_tokens: BTreeMap<(u64, u64), (u64, u64)>,
    }

    impl DeviceAssigningHypervisor for MockHypervisor {
        fn get_phys_mmio_token(&self, base_ipa: u64, size: u64) -> MockHypervisorResult<u64> {
            let token = self.mmio_tokens.get(&(base_ipa, size));

            Ok(*token.ok_or(MockHypervisorError::FailedGetPhysMmioToken)?)
        }

        fn get_phys_iommu_token(
            &self,
            pviommu_id: u64,
            vsid: u64,
        ) -> MockHypervisorResult<(u64, u64)> {
            let token = self.iommu_tokens.get(&(pviommu_id, vsid));

            Ok(*token.ok_or(MockHypervisorError::FailedGetPhysIommuToken)?)
        }
    }

    #[derive(Debug, Eq, PartialEq)]
    struct AssignedDeviceNode {
        path: CString,
        reg: Vec<u8>,
        interrupts: Vec<u8>,
        iommus: Vec<u32>, // pvIOMMU id and vSID
    }

    impl AssignedDeviceNode {
        fn parse(fdt: &Fdt, path: &CStr) -> Result<Self> {
            let Some(node) = fdt.node(path)? else {
                return Err(FdtError::NotFound.into());
            };

            let reg = node.getprop(cstr!("reg"))?.ok_or(DeviceAssignmentError::MalformedReg)?;
            let interrupts = node
                .getprop(cstr!("interrupts"))?
                .ok_or(DeviceAssignmentError::InvalidInterrupts)?;
            let mut iommus = vec![];
            if let Some(mut cells) = node.getprop_cells(cstr!("iommus"))? {
                while let Some(pviommu_id) = cells.next() {
                    // pvIOMMU id
                    let phandle = Phandle::try_from(pviommu_id)?;
                    let pviommu = fdt
                        .node_with_phandle(phandle)?
                        .ok_or(DeviceAssignmentError::MalformedIommus)?;
                    let compatible = pviommu.getprop_str(cstr!("compatible"));
                    if compatible != Ok(Some(cstr!("pkvm,pviommu"))) {
                        return Err(DeviceAssignmentError::MalformedIommus);
                    }
                    let id = pviommu
                        .getprop_u32(cstr!("id"))?
                        .ok_or(DeviceAssignmentError::MalformedIommus)?;
                    iommus.push(id);

                    // vSID
                    let Some(vsid) = cells.next() else {
                        return Err(DeviceAssignmentError::MalformedIommus);
                    };
                    iommus.push(vsid);
                }
            }
            Ok(Self { path: path.into(), reg: reg.into(), interrupts: interrupts.into(), iommus })
        }
    }

    fn collect_pviommus(fdt: &Fdt) -> Result<Vec<u32>> {
        let mut pviommus = BTreeSet::new();
        for pviommu in fdt.compatible_nodes(cstr!("pkvm,pviommu"))? {
            if let Ok(Some(id)) = pviommu.getprop_u32(cstr!("id")) {
                pviommus.insert(id);
            }
        }
        Ok(pviommus.iter().cloned().collect())
    }

    fn into_fdt_prop(native_bytes: Vec<u32>) -> Vec<u8> {
        let mut v = Vec::with_capacity(native_bytes.len() * 4);
        for byte in native_bytes {
            v.extend_from_slice(&byte.to_be_bytes());
        }
        v
    }

    impl From<[u64; 2]> for DeviceReg {
        fn from(fdt_cells: [u64; 2]) -> Self {
            DeviceReg { addr: fdt_cells[0], size: fdt_cells[1] }
        }
    }

    #[test]
    fn device_info_new_without_symbols() {
        let mut fdt_data = fs::read(FDT_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_WITHOUT_SYMBOLS_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor: MockHypervisor = Default::default();
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap();
        assert_eq!(device_info, None);
    }

    #[test]
    fn device_info_new_without_device() {
        let mut fdt_data = fs::read(FDT_WITHOUT_DEVICE_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor: MockHypervisor = Default::default();
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap();
        assert_eq!(device_info, None);
    }

    #[test]
    fn device_info_assigned_info_without_iommus() {
        let mut fdt_data = fs::read(FDT_WITHOUT_IOMMUS_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x9, 0xFF), 0x300)].into(),
            iommu_tokens: BTreeMap::new(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();

        let expected = [AssignedDeviceInfo {
            node_path: CString::new("/bus0/backlight").unwrap(),
            reg: vec![[0x9, 0xFF].into()],
            interrupts: into_fdt_prop(vec![0x0, 0xF, 0x4]),
            iommus: vec![],
        }];

        assert_eq!(device_info.assigned_devices, expected);
    }

    #[test]
    fn device_info_assigned_info() {
        let mut fdt_data = fs::read(FDT_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x9, 0xFF), 0x12F00000)].into(),
            iommu_tokens: [((0x4, 0xFF0), (0x12E40000, 0x3))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();

        let expected = [AssignedDeviceInfo {
            node_path: CString::new("/rng").unwrap(),
            reg: vec![[0x9, 0xFF].into()],
            interrupts: into_fdt_prop(vec![0x0, 0xF, 0x4]),
            iommus: vec![(PvIommu { id: 0x4 }, Vsid(0xFF0))],
        }];

        assert_eq!(device_info.assigned_devices, expected);
    }

    #[test]
    fn device_info_filter() {
        let mut fdt_data = fs::read(FDT_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x9, 0xFF), 0x12F00000)].into(),
            iommu_tokens: [((0x4, 0xFF0), (0x12E40000, 0x3))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();
        device_info.filter(vm_dtbo).unwrap();

        let vm_dtbo = vm_dtbo.as_mut();

        let symbols = vm_dtbo.symbols().unwrap().unwrap();

        let rng = vm_dtbo.node(cstr!("/fragment@0/__overlay__/rng")).unwrap();
        assert_ne!(rng, None);
        let rng_symbol = symbols.getprop_str(cstr!("rng")).unwrap();
        assert_eq!(Some(cstr!("/fragment@0/__overlay__/rng")), rng_symbol);

        let light = vm_dtbo.node(cstr!("/fragment@0/__overlay__/light")).unwrap();
        assert_eq!(light, None);
        let light_symbol = symbols.getprop_str(cstr!("light")).unwrap();
        assert_eq!(None, light_symbol);

        let led = vm_dtbo.node(cstr!("/fragment@0/__overlay__/led")).unwrap();
        assert_eq!(led, None);
        let led_symbol = symbols.getprop_str(cstr!("led")).unwrap();
        assert_eq!(None, led_symbol);

        let backlight = vm_dtbo.node(cstr!("/fragment@0/__overlay__/bus0/backlight")).unwrap();
        assert_eq!(backlight, None);
        let backlight_symbol = symbols.getprop_str(cstr!("backlight")).unwrap();
        assert_eq!(None, backlight_symbol);
    }

    #[test]
    fn device_info_patch() {
        let mut fdt_data = fs::read(FDT_WITHOUT_IOMMUS_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let mut data = vec![0_u8; fdt_data.len() + vm_dtbo_data.len()];
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();
        let platform_dt = Fdt::create_empty_tree(data.as_mut_slice()).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x9, 0xFF), 0x300)].into(),
            iommu_tokens: BTreeMap::new(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();
        device_info.filter(vm_dtbo).unwrap();

        // SAFETY: Damaged VM DTBO wouldn't be used after this unsafe block.
        unsafe {
            platform_dt.apply_overlay(vm_dtbo.as_mut()).unwrap();
        }
        device_info.patch(platform_dt).unwrap();

        let rng_node = platform_dt.node(cstr!("/bus0/backlight")).unwrap().unwrap();
        let phandle = rng_node.getprop_u32(cstr!("phandle")).unwrap();
        assert_ne!(None, phandle);

        // Note: Intentionally not using AssignedDeviceNode for matching all props.
        type FdtResult<T> = libfdt::Result<T>;
        let expected: Vec<(FdtResult<&CStr>, FdtResult<Vec<u8>>)> = vec![
            (Ok(cstr!("android,backlight,ignore-gctrl-reset")), Ok(Vec::new())),
            (Ok(cstr!("compatible")), Ok(Vec::from(*b"android,backlight\0"))),
            (Ok(cstr!("interrupts")), Ok(into_fdt_prop(vec![0x0, 0xF, 0x4]))),
            (Ok(cstr!("iommus")), Ok(Vec::new())),
            (Ok(cstr!("phandle")), Ok(into_fdt_prop(vec![phandle.unwrap()]))),
            (Ok(cstr!("reg")), Ok(into_fdt_prop(vec![0x0, 0x9, 0x0, 0xFF]))),
        ];

        let mut properties: Vec<_> = rng_node
            .properties()
            .unwrap()
            .map(|prop| (prop.name(), prop.value().map(|x| x.into())))
            .collect();
        properties.sort_by(|a, b| {
            let lhs = a.0.unwrap_or_default();
            let rhs = b.0.unwrap_or_default();
            lhs.partial_cmp(rhs).unwrap()
        });

        assert_eq!(properties, expected);
    }

    #[test]
    fn device_info_patch_no_pviommus() {
        let mut fdt_data = fs::read(FDT_WITHOUT_IOMMUS_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let mut data = vec![0_u8; fdt_data.len() + vm_dtbo_data.len()];
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();
        let platform_dt = Fdt::create_empty_tree(data.as_mut_slice()).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x9, 0xFF), 0x300)].into(),
            iommu_tokens: BTreeMap::new(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();
        device_info.filter(vm_dtbo).unwrap();

        // SAFETY: Damaged VM DTBO wouldn't be used after this unsafe block.
        unsafe {
            platform_dt.apply_overlay(vm_dtbo.as_mut()).unwrap();
        }
        device_info.patch(platform_dt).unwrap();

        let compatible = platform_dt.root().next_compatible(cstr!("pkvm,pviommu")).unwrap();
        assert_eq!(None, compatible);

        if let Some(symbols) = platform_dt.symbols().unwrap() {
            for prop in symbols.properties().unwrap() {
                let path = CStr::from_bytes_with_nul(prop.value().unwrap()).unwrap();
                assert_ne!(None, platform_dt.node(path).unwrap());
            }
        }
    }

    #[test]
    fn device_info_overlay_iommu() {
        let mut fdt_data = fs::read(FDT_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();
        let mut platform_dt_data = pvmfw_fdt_template::RAW.to_vec();
        platform_dt_data.resize(pvmfw_fdt_template::RAW.len() * 2, 0);
        let platform_dt = Fdt::from_mut_slice(&mut platform_dt_data).unwrap();
        platform_dt.unpack().unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x9, 0xFF), 0x12F00000)].into(),
            iommu_tokens: [((0x4, 0xFF0), (0x12E40000, 0x3))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();
        device_info.filter(vm_dtbo).unwrap();

        // SAFETY: Damaged VM DTBO wouldn't be used after this unsafe block.
        unsafe {
            platform_dt.apply_overlay(vm_dtbo.as_mut()).unwrap();
        }
        device_info.patch(platform_dt).unwrap();

        let expected = AssignedDeviceNode {
            path: CString::new("/rng").unwrap(),
            reg: into_fdt_prop(vec![0x0, 0x9, 0x0, 0xFF]),
            interrupts: into_fdt_prop(vec![0x0, 0xF, 0x4]),
            iommus: vec![0x4, 0xFF0],
        };

        let node = AssignedDeviceNode::parse(platform_dt, &expected.path);
        assert_eq!(node, Ok(expected));

        let pviommus = collect_pviommus(platform_dt);
        assert_eq!(pviommus, Ok(vec![0x4]));
    }

    #[test]
    fn device_info_multiple_devices_iommus() {
        let mut fdt_data = fs::read(FDT_WITH_MULTIPLE_DEVICES_IOMMUS_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();
        let mut platform_dt_data = pvmfw_fdt_template::RAW.to_vec();
        platform_dt_data.resize(pvmfw_fdt_template::RAW.len() * 2, 0);
        let platform_dt = Fdt::from_mut_slice(&mut platform_dt_data).unwrap();
        platform_dt.unpack().unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [
                ((0x9, 0xFF), 0x12F00000),
                ((0x10000, 0x1000), 0xF00000),
                ((0x20000, 0x1000), 0xF10000),
            ]
            .into(),
            iommu_tokens: [
                ((0x4, 0xFF0), (0x12E40000, 3)),
                ((0x40, 0xFFA), (0x40000, 0x4)),
                ((0x50, 0xFFB), (0x50000, 0x5)),
            ]
            .into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();
        device_info.filter(vm_dtbo).unwrap();

        // SAFETY: Damaged VM DTBO wouldn't be used after this unsafe block.
        unsafe {
            platform_dt.apply_overlay(vm_dtbo.as_mut()).unwrap();
        }
        device_info.patch(platform_dt).unwrap();

        let expected_devices = [
            AssignedDeviceNode {
                path: CString::new("/rng").unwrap(),
                reg: into_fdt_prop(vec![0x0, 0x9, 0x0, 0xFF]),
                interrupts: into_fdt_prop(vec![0x0, 0xF, 0x4]),
                iommus: vec![0x4, 0xFF0],
            },
            AssignedDeviceNode {
                path: CString::new("/light").unwrap(),
                reg: into_fdt_prop(vec![0x0, 0x10000, 0x0, 0x1000, 0x0, 0x20000, 0x0, 0x1000]),
                interrupts: into_fdt_prop(vec![0x0, 0xF, 0x5]),
                iommus: vec![0x40, 0xFFA, 0x50, 0xFFB],
            },
        ];

        for expected in expected_devices {
            let node = AssignedDeviceNode::parse(platform_dt, &expected.path);
            assert_eq!(node, Ok(expected));
        }
        let pviommus = collect_pviommus(platform_dt);
        assert_eq!(pviommus, Ok(vec![0x4, 0x40, 0x50]));
    }

    #[test]
    fn device_info_iommu_sharing() {
        let mut fdt_data = fs::read(FDT_WITH_IOMMU_SHARING).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();
        let mut platform_dt_data = pvmfw_fdt_template::RAW.to_vec();
        platform_dt_data.resize(pvmfw_fdt_template::RAW.len() * 2, 0);
        let platform_dt = Fdt::from_mut_slice(&mut platform_dt_data).unwrap();
        platform_dt.unpack().unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x9, 0xFF), 0x12F00000), ((0x1000, 0x9), 0x12000000)].into(),
            iommu_tokens: [((0x4, 0xFF0), (0x12E40000, 3)), ((0x4, 0xFF1), (0x12E40000, 9))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();
        device_info.filter(vm_dtbo).unwrap();

        // SAFETY: Damaged VM DTBO wouldn't be used after this unsafe block.
        unsafe {
            platform_dt.apply_overlay(vm_dtbo.as_mut()).unwrap();
        }
        device_info.patch(platform_dt).unwrap();

        let expected_devices = [
            AssignedDeviceNode {
                path: CString::new("/rng").unwrap(),
                reg: into_fdt_prop(vec![0x0, 0x9, 0x0, 0xFF]),
                interrupts: into_fdt_prop(vec![0x0, 0xF, 0x4]),
                iommus: vec![0x4, 0xFF0],
            },
            AssignedDeviceNode {
                path: CString::new("/led").unwrap(),
                reg: into_fdt_prop(vec![0x0, 0x1000, 0x0, 0x9]),
                interrupts: into_fdt_prop(vec![0x0, 0xF, 0x5]),
                iommus: vec![0x4, 0xFF1],
            },
        ];

        for expected in expected_devices {
            let node = AssignedDeviceNode::parse(platform_dt, &expected.path);
            assert_eq!(node, Ok(expected));
        }

        let pviommus = collect_pviommus(platform_dt);
        assert_eq!(pviommus, Ok(vec![0x4]));
    }

    #[test]
    fn device_info_iommu_id_conflict() {
        let mut fdt_data = fs::read(FDT_WITH_IOMMU_ID_CONFLICT).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x9, 0xFF), 0x300)].into(),
            iommu_tokens: [((0x4, 0xFF0), (0x12E40000, 0x3))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor);

        assert_eq!(device_info, Err(DeviceAssignmentError::DuplicatedPvIommuIds));
    }

    #[test]
    fn device_info_invalid_reg() {
        let mut fdt_data = fs::read(FDT_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: BTreeMap::new(),
            iommu_tokens: [((0x4, 0xFF0), (0x12E40000, 0x3))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor);

        assert_eq!(device_info, Err(DeviceAssignmentError::InvalidReg(0x9, 0xFF)));
    }

    #[test]
    fn device_info_invalid_reg_out_of_order() {
        let mut fdt_data = fs::read(FDT_WITH_MULTIPLE_REG_IOMMU_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0xF000, 0x1000), 0xF10000), ((0xF100, 0x1000), 0xF00000)].into(),
            iommu_tokens: [((0xFF0, 0xF0), (0x40000, 0x4)), ((0xFF1, 0xF1), (0x50000, 0x5))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor);

        assert_eq!(device_info, Err(DeviceAssignmentError::InvalidPhysReg(0xF10000, 0x1000)));
    }

    #[test]
    fn device_info_invalid_iommus() {
        let mut fdt_data = fs::read(FDT_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x9, 0xFF), 0x12F00000)].into(),
            iommu_tokens: BTreeMap::new(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor);

        assert_eq!(device_info, Err(DeviceAssignmentError::InvalidIommus));
    }

    #[test]
    fn device_info_duplicated_pv_iommus() {
        let mut fdt_data = fs::read(FDT_WITH_DUPLICATED_PVIOMMUS_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x10000, 0x1000), 0xF00000), ((0x20000, 0xFF), 0xF10000)].into(),
            iommu_tokens: [((0xFF, 0xF), (0x40000, 0x4))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor);

        assert_eq!(device_info, Err(DeviceAssignmentError::DuplicatedPvIommuIds));
    }

    #[test]
    fn device_info_duplicated_iommus() {
        let mut fdt_data = fs::read(FDT_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_WITH_DUPLICATED_IOMMUS_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x10000, 0x1000), 0xF00000), ((0x20000, 0xFF), 0xF10000)].into(),
            iommu_tokens: [((0xFF, 0xF), (0x40000, 0x4))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor);

        assert_eq!(device_info, Err(DeviceAssignmentError::UnsupportedIommusDuplication));
    }

    #[test]
    fn device_info_duplicated_iommu_mapping() {
        let mut fdt_data = fs::read(FDT_WITH_MULTIPLE_REG_IOMMU_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0xF000, 0x1000), 0xF00000), ((0xF100, 0x1000), 0xF10000)].into(),
            iommu_tokens: [((0xFF0, 0xF0), (0x40000, 0x4)), ((0xFF1, 0xF1), (0x40000, 0x4))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor);

        assert_eq!(device_info, Err(DeviceAssignmentError::InvalidIommus));
    }

    #[test]
    fn device_info_overlaps_pvmfw() {
        let mut fdt_data = fs::read(FDT_WITH_DEVICE_OVERLAPPING_PVMFW).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0x7fee0000, 0x1000), 0xF00000)].into(),
            iommu_tokens: [((0xFF, 0xF), (0x40000, 0x4))].into(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor);

        assert_eq!(device_info, Err(DeviceAssignmentError::InvalidReg(0x7fee0000, 0x1000)));
    }

    #[test]
    fn device_assignment_clean() {
        let mut platform_dt_data = pvmfw_fdt_template::RAW.to_vec();
        let platform_dt = Fdt::from_mut_slice(&mut platform_dt_data).unwrap();

        let compatible = platform_dt.root().next_compatible(cstr!("pkvm,pviommu"));
        assert_ne!(None, compatible.unwrap());

        clean(platform_dt).unwrap();

        let compatible = platform_dt.root().next_compatible(cstr!("pkvm,pviommu"));
        assert_eq!(Ok(None), compatible);
    }

    #[test]
    fn device_info_dependency() {
        let mut fdt_data = fs::read(FDT_WITH_DEPENDENCY_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_WITH_DEPENDENCIES_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();
        let mut platform_dt_data = pvmfw_fdt_template::RAW.to_vec();
        platform_dt_data.resize(pvmfw_fdt_template::RAW.len() * 2, 0);
        let platform_dt = Fdt::from_mut_slice(&mut platform_dt_data).unwrap();
        platform_dt.unpack().unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0xFF000, 0x1), 0xF000)].into(),
            iommu_tokens: Default::default(),
        };

        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();
        device_info.filter(vm_dtbo).unwrap();

        // SAFETY: Damaged VM DTBO wouldn't be used after this unsafe block.
        unsafe {
            platform_dt.apply_overlay(vm_dtbo.as_mut()).unwrap();
        }
        device_info.patch(platform_dt).unwrap();

        let expected = Dts::from_dtb(Path::new(EXPECTED_FDT_WITH_DEPENDENCY_FILE_PATH)).unwrap();
        let platform_dt = Dts::from_fdt(platform_dt).unwrap();

        assert_eq!(expected, platform_dt);
    }

    #[test]
    fn device_info_multiple_dependencies() {
        let mut fdt_data = fs::read(FDT_WITH_MULTIPLE_DEPENDENCIES_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_WITH_DEPENDENCIES_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();
        let mut platform_dt_data = pvmfw_fdt_template::RAW.to_vec();
        platform_dt_data.resize(pvmfw_fdt_template::RAW.len() * 2, 0);
        let platform_dt = Fdt::from_mut_slice(&mut platform_dt_data).unwrap();
        platform_dt.unpack().unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0xFF000, 0x1), 0xF000), ((0xFF100, 0x1), 0xF100)].into(),
            iommu_tokens: Default::default(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();
        device_info.filter(vm_dtbo).unwrap();

        // SAFETY: Damaged VM DTBO wouldn't be used after this unsafe block.
        unsafe {
            platform_dt.apply_overlay(vm_dtbo.as_mut()).unwrap();
        }
        device_info.patch(platform_dt).unwrap();

        let expected =
            Dts::from_dtb(Path::new(EXPECTED_FDT_WITH_MULTIPLE_DEPENDENCIES_FILE_PATH)).unwrap();
        let platform_dt = Dts::from_fdt(platform_dt).unwrap();

        assert_eq!(expected, platform_dt);
    }

    #[test]
    fn device_info_dependency_loop() {
        let mut fdt_data = fs::read(FDT_WITH_DEPENDENCY_LOOP_FILE_PATH).unwrap();
        let mut vm_dtbo_data = fs::read(VM_DTBO_WITH_DEPENDENCIES_FILE_PATH).unwrap();
        let fdt = Fdt::from_mut_slice(&mut fdt_data).unwrap();
        let vm_dtbo = VmDtbo::from_mut_slice(&mut vm_dtbo_data).unwrap();
        let mut platform_dt_data = pvmfw_fdt_template::RAW.to_vec();
        platform_dt_data.resize(pvmfw_fdt_template::RAW.len() * 2, 0);
        let platform_dt = Fdt::from_mut_slice(&mut platform_dt_data).unwrap();
        platform_dt.unpack().unwrap();

        let hypervisor = MockHypervisor {
            mmio_tokens: [((0xFF200, 0x1), 0xF200)].into(),
            iommu_tokens: Default::default(),
        };
        let device_info = DeviceAssignmentInfo::parse(fdt, vm_dtbo, &hypervisor).unwrap().unwrap();
        device_info.filter(vm_dtbo).unwrap();

        // SAFETY: Damaged VM DTBO wouldn't be used after this unsafe block.
        unsafe {
            platform_dt.apply_overlay(vm_dtbo.as_mut()).unwrap();
        }
        device_info.patch(platform_dt).unwrap();

        let expected =
            Dts::from_dtb(Path::new(EXPECTED_FDT_WITH_DEPENDENCY_LOOP_FILE_PATH)).unwrap();
        let platform_dt = Dts::from_fdt(platform_dt).unwrap();

        assert_eq!(expected, platform_dt);
    }
}
