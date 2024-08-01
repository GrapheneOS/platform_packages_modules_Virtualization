// Copyright 2022, The Android Open Source Project
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

//! High-level FDT functions.

use crate::bootargs::BootArgsIterator;
use crate::device_assignment::{self, DeviceAssignmentInfo, VmDtbo};
use crate::helpers::GUEST_PAGE_SIZE;
use crate::Box;
use crate::RebootReason;
use alloc::collections::BTreeMap;
use alloc::ffi::CString;
use alloc::format;
use alloc::vec::Vec;
use core::cmp::max;
use core::cmp::min;
use core::ffi::CStr;
use core::fmt;
use core::mem::size_of;
use core::ops::Range;
use cstr::cstr;
use fdtpci::PciMemoryFlags;
use fdtpci::PciRangeType;
use libfdt::AddressRange;
use libfdt::CellIterator;
use libfdt::Fdt;
use libfdt::FdtError;
use libfdt::FdtNode;
use libfdt::FdtNodeMut;
use libfdt::Phandle;
use log::debug;
use log::error;
use log::info;
use log::warn;
use static_assertions::const_assert;
use tinyvec::ArrayVec;
use vmbase::fdt::SwiotlbInfo;
use vmbase::hyp;
use vmbase::layout::{crosvm::MEM_START, MAX_VIRT_ADDR};
use vmbase::memory::SIZE_4KB;
use vmbase::util::flatten;
use vmbase::util::RangeExt as _;
use zerocopy::AsBytes as _;

/// An enumeration of errors that can occur during the FDT validation.
#[derive(Clone, Debug)]
pub enum FdtValidationError {
    /// Invalid CPU count.
    InvalidCpuCount(usize),
    /// Invalid VCpufreq Range.
    InvalidVcpufreq(u64, u64),
    /// Forbidden /avf/untrusted property.
    ForbiddenUntrustedProp(&'static CStr),
}

impl fmt::Display for FdtValidationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::InvalidCpuCount(num_cpus) => write!(f, "Invalid CPU count: {num_cpus}"),
            Self::InvalidVcpufreq(addr, size) => {
                write!(f, "Invalid vcpufreq region: ({addr:#x}, {size:#x})")
            }
            Self::ForbiddenUntrustedProp(name) => {
                write!(f, "Forbidden /avf/untrusted property '{name:?}'")
            }
        }
    }
}

/// Extract from /config the address range containing the pre-loaded kernel. Absence of /config is
/// not an error.
fn read_kernel_range_from(fdt: &Fdt) -> libfdt::Result<Option<Range<usize>>> {
    let addr = cstr!("kernel-address");
    let size = cstr!("kernel-size");

    if let Some(config) = fdt.node(cstr!("/config"))? {
        if let (Some(addr), Some(size)) = (config.getprop_u32(addr)?, config.getprop_u32(size)?) {
            let addr = addr as usize;
            let size = size as usize;

            return Ok(Some(addr..(addr + size)));
        }
    }

    Ok(None)
}

/// Extract from /chosen the address range containing the pre-loaded ramdisk. Absence is not an
/// error as there can be initrd-less VM.
fn read_initrd_range_from(fdt: &Fdt) -> libfdt::Result<Option<Range<usize>>> {
    let start = cstr!("linux,initrd-start");
    let end = cstr!("linux,initrd-end");

    if let Some(chosen) = fdt.chosen()? {
        if let (Some(start), Some(end)) = (chosen.getprop_u32(start)?, chosen.getprop_u32(end)?) {
            return Ok(Some((start as usize)..(end as usize)));
        }
    }

    Ok(None)
}

fn patch_initrd_range(fdt: &mut Fdt, initrd_range: &Range<usize>) -> libfdt::Result<()> {
    let start = u32::try_from(initrd_range.start).unwrap();
    let end = u32::try_from(initrd_range.end).unwrap();

    let mut node = fdt.chosen_mut()?.ok_or(FdtError::NotFound)?;
    node.setprop(cstr!("linux,initrd-start"), &start.to_be_bytes())?;
    node.setprop(cstr!("linux,initrd-end"), &end.to_be_bytes())?;
    Ok(())
}

fn read_bootargs_from(fdt: &Fdt) -> libfdt::Result<Option<CString>> {
    if let Some(chosen) = fdt.chosen()? {
        if let Some(bootargs) = chosen.getprop_str(cstr!("bootargs"))? {
            // We need to copy the string to heap because the original fdt will be invalidated
            // by the templated DT
            let copy = CString::new(bootargs.to_bytes()).map_err(|_| FdtError::BadValue)?;
            return Ok(Some(copy));
        }
    }
    Ok(None)
}

fn patch_bootargs(fdt: &mut Fdt, bootargs: &CStr) -> libfdt::Result<()> {
    let mut node = fdt.chosen_mut()?.ok_or(FdtError::NotFound)?;
    // This function is called before the verification is done. So, we just copy the bootargs to
    // the new FDT unmodified. This will be filtered again in the modify_for_next_stage function
    // if the VM is not debuggable.
    node.setprop(cstr!("bootargs"), bootargs.to_bytes_with_nul())
}

/// Reads and validates the memory range in the DT.
///
/// Only one memory range is expected with the crosvm setup for now.
fn read_and_validate_memory_range(fdt: &Fdt) -> Result<Range<usize>, RebootReason> {
    let mut memory = fdt.memory().map_err(|e| {
        error!("Failed to read memory range from DT: {e}");
        RebootReason::InvalidFdt
    })?;
    let range = memory.next().ok_or_else(|| {
        error!("The /memory node in the DT contains no range.");
        RebootReason::InvalidFdt
    })?;
    if memory.next().is_some() {
        warn!(
            "The /memory node in the DT contains more than one memory range, \
             while only one is expected."
        );
    }
    let base = range.start;
    if base != MEM_START {
        error!("Memory base address {:#x} is not {:#x}", base, MEM_START);
        return Err(RebootReason::InvalidFdt);
    }

    let size = range.len();
    if size % GUEST_PAGE_SIZE != 0 {
        error!("Memory size {:#x} is not a multiple of page size {:#x}", size, GUEST_PAGE_SIZE);
        return Err(RebootReason::InvalidFdt);
    }

    if size == 0 {
        error!("Memory size is 0");
        return Err(RebootReason::InvalidFdt);
    }
    Ok(range)
}

fn patch_memory_range(fdt: &mut Fdt, memory_range: &Range<usize>) -> libfdt::Result<()> {
    let addr = u64::try_from(MEM_START).unwrap();
    let size = u64::try_from(memory_range.len()).unwrap();
    fdt.node_mut(cstr!("/memory"))?
        .ok_or(FdtError::NotFound)?
        .setprop_inplace(cstr!("reg"), [addr.to_be(), size.to_be()].as_bytes())
}

#[derive(Debug, Default)]
struct CpuInfo {
    opptable_info: Option<ArrayVec<[u64; CpuInfo::MAX_OPPTABLES]>>,
    cpu_capacity: Option<u32>,
}

impl CpuInfo {
    const MAX_OPPTABLES: usize = 20;
}

fn read_opp_info_from(
    opp_node: FdtNode,
) -> libfdt::Result<ArrayVec<[u64; CpuInfo::MAX_OPPTABLES]>> {
    let mut table = ArrayVec::new();
    let mut opp_nodes = opp_node.subnodes()?;
    for subnode in opp_nodes.by_ref().take(table.capacity()) {
        let prop = subnode.getprop_u64(cstr!("opp-hz"))?.ok_or(FdtError::NotFound)?;
        table.push(prop);
    }

    if opp_nodes.next().is_some() {
        warn!("OPP table has more than {} entries: discarding extra nodes.", table.capacity());
    }

    Ok(table)
}

#[derive(Debug, Default)]
struct ClusterTopology {
    // TODO: Support multi-level clusters & threads.
    cores: [Option<usize>; ClusterTopology::MAX_CORES_PER_CLUSTER],
}

impl ClusterTopology {
    const MAX_CORES_PER_CLUSTER: usize = 10;
}

#[derive(Debug, Default)]
struct CpuTopology {
    // TODO: Support sockets.
    clusters: [Option<ClusterTopology>; CpuTopology::MAX_CLUSTERS],
}

impl CpuTopology {
    const MAX_CLUSTERS: usize = 3;
}

fn read_cpu_map_from(fdt: &Fdt) -> libfdt::Result<Option<BTreeMap<Phandle, (usize, usize)>>> {
    let Some(cpu_map) = fdt.node(cstr!("/cpus/cpu-map"))? else {
        return Ok(None);
    };

    let mut topology = BTreeMap::new();
    for n in 0..CpuTopology::MAX_CLUSTERS {
        let name = CString::new(format!("cluster{n}")).unwrap();
        let Some(cluster) = cpu_map.subnode(&name)? else {
            break;
        };
        for m in 0..ClusterTopology::MAX_CORES_PER_CLUSTER {
            let name = CString::new(format!("core{m}")).unwrap();
            let Some(core) = cluster.subnode(&name)? else {
                break;
            };
            let cpu = core.getprop_u32(cstr!("cpu"))?.ok_or(FdtError::NotFound)?;
            let prev = topology.insert(cpu.try_into()?, (n, m));
            if prev.is_some() {
                return Err(FdtError::BadValue);
            }
        }
    }

    Ok(Some(topology))
}

fn read_cpu_info_from(
    fdt: &Fdt,
) -> libfdt::Result<(ArrayVec<[CpuInfo; DeviceTreeInfo::MAX_CPUS]>, Option<CpuTopology>)> {
    let mut cpus = ArrayVec::new();

    let cpu_map = read_cpu_map_from(fdt)?;
    let mut topology: CpuTopology = Default::default();

    let mut cpu_nodes = fdt.compatible_nodes(cstr!("arm,armv8"))?;
    for (idx, cpu) in cpu_nodes.by_ref().take(cpus.capacity()).enumerate() {
        let cpu_capacity = cpu.getprop_u32(cstr!("capacity-dmips-mhz"))?;
        let opp_phandle = cpu.getprop_u32(cstr!("operating-points-v2"))?;
        let opptable_info = if let Some(phandle) = opp_phandle {
            let phandle = phandle.try_into()?;
            let node = fdt.node_with_phandle(phandle)?.ok_or(FdtError::NotFound)?;
            Some(read_opp_info_from(node)?)
        } else {
            None
        };
        let info = CpuInfo { opptable_info, cpu_capacity };
        cpus.push(info);

        if let Some(ref cpu_map) = cpu_map {
            let phandle = cpu.get_phandle()?.ok_or(FdtError::NotFound)?;
            let (cluster, core_idx) = cpu_map.get(&phandle).ok_or(FdtError::BadValue)?;
            let cluster = topology.clusters[*cluster].get_or_insert(Default::default());
            if cluster.cores[*core_idx].is_some() {
                return Err(FdtError::BadValue);
            }
            cluster.cores[*core_idx] = Some(idx);
        }
    }

    if cpu_nodes.next().is_some() {
        warn!("DT has more than {} CPU nodes: discarding extra nodes.", cpus.capacity());
    }

    Ok((cpus, cpu_map.map(|_| topology)))
}

fn validate_cpu_info(cpus: &[CpuInfo]) -> Result<(), FdtValidationError> {
    if cpus.is_empty() {
        return Err(FdtValidationError::InvalidCpuCount(0));
    }
    Ok(())
}

fn read_vcpufreq_info(fdt: &Fdt) -> libfdt::Result<Option<VcpufreqInfo>> {
    let mut nodes = fdt.compatible_nodes(cstr!("virtual,android-v-only-cpufreq"))?;
    let Some(node) = nodes.next() else {
        return Ok(None);
    };

    if nodes.next().is_some() {
        warn!("DT has more than 1 cpufreq node: discarding extra nodes.");
    }

    let mut regs = node.reg()?.ok_or(FdtError::NotFound)?;
    let reg = regs.next().ok_or(FdtError::NotFound)?;
    let size = reg.size.ok_or(FdtError::NotFound)?;

    Ok(Some(VcpufreqInfo { addr: reg.addr, size }))
}

fn validate_vcpufreq_info(
    vcpufreq_info: &VcpufreqInfo,
    cpus: &[CpuInfo],
) -> Result<(), FdtValidationError> {
    const VCPUFREQ_BASE_ADDR: u64 = 0x1040000;
    const VCPUFREQ_SIZE_PER_CPU: u64 = 0x8;

    let base = vcpufreq_info.addr;
    let size = vcpufreq_info.size;
    let expected_size = VCPUFREQ_SIZE_PER_CPU * cpus.len() as u64;

    if (base, size) != (VCPUFREQ_BASE_ADDR, expected_size) {
        return Err(FdtValidationError::InvalidVcpufreq(base, size));
    }

    Ok(())
}

fn patch_opptable(
    node: FdtNodeMut,
    opptable: Option<ArrayVec<[u64; CpuInfo::MAX_OPPTABLES]>>,
) -> libfdt::Result<()> {
    let oppcompat = cstr!("operating-points-v2");
    let next = node.next_compatible(oppcompat)?.ok_or(FdtError::NoSpace)?;

    let Some(opptable) = opptable else {
        return next.nop();
    };

    let mut next_subnode = next.first_subnode()?;

    for entry in opptable {
        let mut subnode = next_subnode.ok_or(FdtError::NoSpace)?;
        subnode.setprop_inplace(cstr!("opp-hz"), &entry.to_be_bytes())?;
        next_subnode = subnode.next_subnode()?;
    }

    while let Some(current) = next_subnode {
        next_subnode = current.delete_and_next_subnode()?;
    }

    Ok(())
}

// TODO(ptosi): Rework FdtNodeMut and replace this function.
fn get_nth_compatible<'a>(
    fdt: &'a mut Fdt,
    n: usize,
    compat: &CStr,
) -> libfdt::Result<Option<FdtNodeMut<'a>>> {
    let mut node = fdt.root_mut().next_compatible(compat)?;
    for _ in 0..n {
        node = node.ok_or(FdtError::NoSpace)?.next_compatible(compat)?;
    }
    Ok(node)
}

fn patch_cpus(
    fdt: &mut Fdt,
    cpus: &[CpuInfo],
    topology: &Option<CpuTopology>,
) -> libfdt::Result<()> {
    const COMPAT: &CStr = cstr!("arm,armv8");
    let mut cpu_phandles = Vec::new();
    for (idx, cpu) in cpus.iter().enumerate() {
        let mut cur = get_nth_compatible(fdt, idx, COMPAT)?.ok_or(FdtError::NoSpace)?;
        let phandle = cur.as_node().get_phandle()?.unwrap();
        cpu_phandles.push(phandle);
        if let Some(cpu_capacity) = cpu.cpu_capacity {
            cur.setprop_inplace(cstr!("capacity-dmips-mhz"), &cpu_capacity.to_be_bytes())?;
        }
        patch_opptable(cur, cpu.opptable_info)?;
    }
    let mut next = get_nth_compatible(fdt, cpus.len(), COMPAT)?;
    while let Some(current) = next {
        next = current.delete_and_next_compatible(COMPAT)?;
    }

    if let Some(topology) = topology {
        for (n, cluster) in topology.clusters.iter().enumerate() {
            let path = CString::new(format!("/cpus/cpu-map/cluster{n}")).unwrap();
            let cluster_node = fdt.node_mut(&path)?.unwrap();
            if let Some(cluster) = cluster {
                let mut iter = cluster_node.first_subnode()?;
                for core in cluster.cores {
                    let mut core_node = iter.unwrap();
                    iter = if let Some(core_idx) = core {
                        let phandle = *cpu_phandles.get(core_idx).unwrap();
                        let value = u32::from(phandle).to_be_bytes();
                        core_node.setprop_inplace(cstr!("cpu"), &value)?;
                        core_node.next_subnode()?
                    } else {
                        core_node.delete_and_next_subnode()?
                    };
                }
                assert!(iter.is_none());
            } else {
                cluster_node.nop()?;
            }
        }
    } else {
        fdt.node_mut(cstr!("/cpus/cpu-map"))?.unwrap().nop()?;
    }

    Ok(())
}

/// Reads the /avf/untrusted DT node, which the host can use to pass properties (no subnodes) to
/// the guest that don't require being validated by pvmfw.
fn parse_untrusted_props(fdt: &Fdt) -> libfdt::Result<BTreeMap<CString, Vec<u8>>> {
    let mut props = BTreeMap::new();
    if let Some(node) = fdt.node(cstr!("/avf/untrusted"))? {
        for property in node.properties()? {
            let name = property.name()?;
            let value = property.value()?;
            props.insert(CString::from(name), value.to_vec());
        }
        if node.subnodes()?.next().is_some() {
            warn!("Discarding unexpected /avf/untrusted subnodes.");
        }
    }

    Ok(props)
}

/// Read candidate properties' names from DT which could be overlaid
fn parse_vm_ref_dt(fdt: &Fdt) -> libfdt::Result<BTreeMap<CString, Vec<u8>>> {
    let mut property_map = BTreeMap::new();
    if let Some(avf_node) = fdt.node(cstr!("/avf"))? {
        for property in avf_node.properties()? {
            let name = property.name()?;
            let value = property.value()?;
            property_map.insert(
                CString::new(name.to_bytes()).map_err(|_| FdtError::BadValue)?,
                value.to_vec(),
            );
        }
    }
    Ok(property_map)
}

fn validate_untrusted_props(props: &BTreeMap<CString, Vec<u8>>) -> Result<(), FdtValidationError> {
    const FORBIDDEN_PROPS: &[&CStr] =
        &[cstr!("compatible"), cstr!("linux,phandle"), cstr!("phandle")];

    for name in FORBIDDEN_PROPS {
        if props.contains_key(*name) {
            return Err(FdtValidationError::ForbiddenUntrustedProp(name));
        }
    }

    Ok(())
}

/// Overlay VM reference DT into VM DT based on the props_info. Property is overlaid in vm_dt only
/// when it exists both in vm_ref_dt and props_info. If the values mismatch, it returns error.
fn validate_vm_ref_dt(
    vm_dt: &mut Fdt,
    vm_ref_dt: &Fdt,
    props_info: &BTreeMap<CString, Vec<u8>>,
) -> libfdt::Result<()> {
    let root_vm_dt = vm_dt.root_mut();
    let mut avf_vm_dt = root_vm_dt.add_subnode(cstr!("avf"))?;
    // TODO(b/318431677): Validate nodes beyond /avf.
    let avf_node = vm_ref_dt.node(cstr!("/avf"))?.ok_or(FdtError::NotFound)?;
    for (name, value) in props_info.iter() {
        if let Some(ref_value) = avf_node.getprop(name)? {
            if value != ref_value {
                error!(
                    "Property mismatches while applying overlay VM reference DT. \
                    Name:{:?}, Value from host as hex:{:x?}, Value from VM reference DT as hex:{:x?}",
                    name, value, ref_value
                );
                return Err(FdtError::BadValue);
            }
            avf_vm_dt.setprop(name, ref_value)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct PciInfo {
    ranges: [PciAddrRange; 2],
    irq_masks: ArrayVec<[PciIrqMask; PciInfo::MAX_IRQS]>,
    irq_maps: ArrayVec<[PciIrqMap; PciInfo::MAX_IRQS]>,
}

impl PciInfo {
    const IRQ_MASK_CELLS: usize = 4;
    const IRQ_MAP_CELLS: usize = 10;
    const MAX_IRQS: usize = 16;
}

type PciAddrRange = AddressRange<(u32, u64), u64, u64>;
type PciIrqMask = [u32; PciInfo::IRQ_MASK_CELLS];
type PciIrqMap = [u32; PciInfo::IRQ_MAP_CELLS];

/// Iterator that takes N cells as a chunk
struct CellChunkIterator<'a, const N: usize> {
    cells: CellIterator<'a>,
}

impl<'a, const N: usize> CellChunkIterator<'a, N> {
    fn new(cells: CellIterator<'a>) -> Self {
        Self { cells }
    }
}

impl<'a, const N: usize> Iterator for CellChunkIterator<'a, N> {
    type Item = [u32; N];
    fn next(&mut self) -> Option<Self::Item> {
        let mut ret: Self::Item = [0; N];
        for i in ret.iter_mut() {
            *i = self.cells.next()?;
        }
        Some(ret)
    }
}

/// Read pci host controller ranges, irq maps, and irq map masks from DT
fn read_pci_info_from(fdt: &Fdt) -> libfdt::Result<PciInfo> {
    let node =
        fdt.compatible_nodes(cstr!("pci-host-cam-generic"))?.next().ok_or(FdtError::NotFound)?;

    let mut ranges = node.ranges::<(u32, u64), u64, u64>()?.ok_or(FdtError::NotFound)?;
    let range0 = ranges.next().ok_or(FdtError::NotFound)?;
    let range1 = ranges.next().ok_or(FdtError::NotFound)?;

    let irq_masks = node.getprop_cells(cstr!("interrupt-map-mask"))?.ok_or(FdtError::NotFound)?;
    let mut chunks = CellChunkIterator::<{ PciInfo::IRQ_MASK_CELLS }>::new(irq_masks);
    let irq_masks = (&mut chunks).take(PciInfo::MAX_IRQS).collect();

    if chunks.next().is_some() {
        warn!("Input DT has more than {} PCI entries!", PciInfo::MAX_IRQS);
        return Err(FdtError::NoSpace);
    }

    let irq_maps = node.getprop_cells(cstr!("interrupt-map"))?.ok_or(FdtError::NotFound)?;
    let mut chunks = CellChunkIterator::<{ PciInfo::IRQ_MAP_CELLS }>::new(irq_maps);
    let irq_maps = (&mut chunks).take(PciInfo::MAX_IRQS).collect();

    if chunks.next().is_some() {
        warn!("Input DT has more than {} PCI entries!", PciInfo::MAX_IRQS);
        return Err(FdtError::NoSpace);
    }

    Ok(PciInfo { ranges: [range0, range1], irq_masks, irq_maps })
}

fn validate_pci_info(pci_info: &PciInfo, memory_range: &Range<usize>) -> Result<(), RebootReason> {
    for range in pci_info.ranges.iter() {
        validate_pci_addr_range(range, memory_range)?;
    }
    for irq_mask in pci_info.irq_masks.iter() {
        validate_pci_irq_mask(irq_mask)?;
    }
    for (idx, irq_map) in pci_info.irq_maps.iter().enumerate() {
        validate_pci_irq_map(irq_map, idx)?;
    }
    Ok(())
}

fn validate_pci_addr_range(
    range: &PciAddrRange,
    memory_range: &Range<usize>,
) -> Result<(), RebootReason> {
    let mem_flags = PciMemoryFlags(range.addr.0);
    let range_type = mem_flags.range_type();
    let bus_addr = range.addr.1;
    let cpu_addr = range.parent_addr;
    let size = range.size;

    if range_type != PciRangeType::Memory64 {
        error!("Invalid range type {:?} for bus address {:#x} in PCI node", range_type, bus_addr);
        return Err(RebootReason::InvalidFdt);
    }
    // Enforce ID bus-to-cpu mappings, as used by crosvm.
    if bus_addr != cpu_addr {
        error!("PCI bus address: {:#x} is different from CPU address: {:#x}", bus_addr, cpu_addr);
        return Err(RebootReason::InvalidFdt);
    }

    let Some(bus_end) = bus_addr.checked_add(size) else {
        error!("PCI address range size {:#x} overflows", size);
        return Err(RebootReason::InvalidFdt);
    };
    if bus_end > MAX_VIRT_ADDR.try_into().unwrap() {
        error!("PCI address end {:#x} is outside of translatable range", bus_end);
        return Err(RebootReason::InvalidFdt);
    }

    let memory_start = memory_range.start.try_into().unwrap();
    let memory_end = memory_range.end.try_into().unwrap();

    if max(bus_addr, memory_start) < min(bus_end, memory_end) {
        error!(
            "PCI address range {:#x}-{:#x} overlaps with main memory range {:#x}-{:#x}",
            bus_addr, bus_end, memory_start, memory_end
        );
        return Err(RebootReason::InvalidFdt);
    }

    Ok(())
}

fn validate_pci_irq_mask(irq_mask: &PciIrqMask) -> Result<(), RebootReason> {
    const IRQ_MASK_ADDR_HI: u32 = 0xf800;
    const IRQ_MASK_ADDR_ME: u32 = 0x0;
    const IRQ_MASK_ADDR_LO: u32 = 0x0;
    const IRQ_MASK_ANY_IRQ: u32 = 0x7;
    const EXPECTED: PciIrqMask =
        [IRQ_MASK_ADDR_HI, IRQ_MASK_ADDR_ME, IRQ_MASK_ADDR_LO, IRQ_MASK_ANY_IRQ];
    if *irq_mask != EXPECTED {
        error!("Invalid PCI irq mask {:#?}", irq_mask);
        return Err(RebootReason::InvalidFdt);
    }
    Ok(())
}

fn validate_pci_irq_map(irq_map: &PciIrqMap, idx: usize) -> Result<(), RebootReason> {
    const PCI_DEVICE_IDX: usize = 11;
    const PCI_IRQ_ADDR_ME: u32 = 0;
    const PCI_IRQ_ADDR_LO: u32 = 0;
    const PCI_IRQ_INTC: u32 = 1;
    const AARCH64_IRQ_BASE: u32 = 4; // from external/crosvm/aarch64/src/lib.rs
    const GIC_SPI: u32 = 0;
    const IRQ_TYPE_LEVEL_HIGH: u32 = 4;

    let pci_addr = (irq_map[0], irq_map[1], irq_map[2]);
    let pci_irq_number = irq_map[3];
    let _controller_phandle = irq_map[4]; // skipped.
    let gic_addr = (irq_map[5], irq_map[6]); // address-cells is <2> for GIC
                                             // interrupt-cells is <3> for GIC
    let gic_peripheral_interrupt_type = irq_map[7];
    let gic_irq_number = irq_map[8];
    let gic_irq_type = irq_map[9];

    let phys_hi: u32 = (0x1 << PCI_DEVICE_IDX) * (idx + 1) as u32;
    let expected_pci_addr = (phys_hi, PCI_IRQ_ADDR_ME, PCI_IRQ_ADDR_LO);

    if pci_addr != expected_pci_addr {
        error!("PCI device address {:#x} {:#x} {:#x} in interrupt-map is different from expected address \
               {:#x} {:#x} {:#x}",
               pci_addr.0, pci_addr.1, pci_addr.2, expected_pci_addr.0, expected_pci_addr.1, expected_pci_addr.2);
        return Err(RebootReason::InvalidFdt);
    }

    if pci_irq_number != PCI_IRQ_INTC {
        error!(
            "PCI INT# {:#x} in interrupt-map is different from expected value {:#x}",
            pci_irq_number, PCI_IRQ_INTC
        );
        return Err(RebootReason::InvalidFdt);
    }

    if gic_addr != (0, 0) {
        error!(
            "GIC address {:#x} {:#x} in interrupt-map is different from expected address \
               {:#x} {:#x}",
            gic_addr.0, gic_addr.1, 0, 0
        );
        return Err(RebootReason::InvalidFdt);
    }

    if gic_peripheral_interrupt_type != GIC_SPI {
        error!("GIC peripheral interrupt type {:#x} in interrupt-map is different from expected value \
               {:#x}", gic_peripheral_interrupt_type, GIC_SPI);
        return Err(RebootReason::InvalidFdt);
    }

    let irq_nr: u32 = AARCH64_IRQ_BASE + (idx as u32);
    if gic_irq_number != irq_nr {
        error!(
            "GIC irq number {:#x} in interrupt-map is unexpected. Expected {:#x}",
            gic_irq_number, irq_nr
        );
        return Err(RebootReason::InvalidFdt);
    }

    if gic_irq_type != IRQ_TYPE_LEVEL_HIGH {
        error!(
            "IRQ type in {:#x} is invalid. Must be LEVEL_HIGH {:#x}",
            gic_irq_type, IRQ_TYPE_LEVEL_HIGH
        );
        return Err(RebootReason::InvalidFdt);
    }
    Ok(())
}

fn patch_pci_info(fdt: &mut Fdt, pci_info: &PciInfo) -> libfdt::Result<()> {
    let mut node =
        fdt.root_mut().next_compatible(cstr!("pci-host-cam-generic"))?.ok_or(FdtError::NotFound)?;

    let irq_masks_size = pci_info.irq_masks.len() * size_of::<PciIrqMask>();
    node.trimprop(cstr!("interrupt-map-mask"), irq_masks_size)?;

    let irq_maps_size = pci_info.irq_maps.len() * size_of::<PciIrqMap>();
    node.trimprop(cstr!("interrupt-map"), irq_maps_size)?;

    node.setprop_inplace(
        cstr!("ranges"),
        flatten(&[pci_info.ranges[0].to_cells(), pci_info.ranges[1].to_cells()]),
    )
}

#[derive(Default, Debug)]
struct SerialInfo {
    addrs: ArrayVec<[u64; Self::MAX_SERIALS]>,
}

impl SerialInfo {
    const MAX_SERIALS: usize = 4;
}

fn read_serial_info_from(fdt: &Fdt) -> libfdt::Result<SerialInfo> {
    let mut addrs = ArrayVec::new();

    let mut serial_nodes = fdt.compatible_nodes(cstr!("ns16550a"))?;
    for node in serial_nodes.by_ref().take(addrs.capacity()) {
        let reg = node.first_reg()?;
        addrs.push(reg.addr);
    }
    if serial_nodes.next().is_some() {
        warn!("DT has more than {} UART nodes: discarding extra nodes.", addrs.capacity());
    }

    Ok(SerialInfo { addrs })
}

#[derive(Default, Debug, PartialEq)]
struct WdtInfo {
    addr: u64,
    size: u64,
    irq: [u32; WdtInfo::IRQ_CELLS],
}

impl WdtInfo {
    const IRQ_CELLS: usize = 3;
    const IRQ_NR: u32 = 0xf;
    const ADDR: u64 = 0x3000;
    const SIZE: u64 = 0x1000;
    const GIC_PPI: u32 = 1;
    const IRQ_TYPE_EDGE_RISING: u32 = 1;
    const GIC_FDT_IRQ_PPI_CPU_SHIFT: u32 = 8;
    // TODO(b/350498812): Rework this for >8 vCPUs.
    const GIC_FDT_IRQ_PPI_CPU_MASK: u32 = 0xff << Self::GIC_FDT_IRQ_PPI_CPU_SHIFT;

    const fn get_expected(num_cpus: usize) -> Self {
        Self {
            addr: Self::ADDR,
            size: Self::SIZE,
            irq: [
                Self::GIC_PPI,
                Self::IRQ_NR,
                ((((1 << num_cpus) - 1) << Self::GIC_FDT_IRQ_PPI_CPU_SHIFT)
                    & Self::GIC_FDT_IRQ_PPI_CPU_MASK)
                    | Self::IRQ_TYPE_EDGE_RISING,
            ],
        }
    }
}

fn read_wdt_info_from(fdt: &Fdt) -> libfdt::Result<WdtInfo> {
    let mut node_iter = fdt.compatible_nodes(cstr!("qemu,vcpu-stall-detector"))?;
    let node = node_iter.next().ok_or(FdtError::NotFound)?;
    let mut ranges = node.reg()?.ok_or(FdtError::NotFound)?;

    let reg = ranges.next().ok_or(FdtError::NotFound)?;
    let size = reg.size.ok_or(FdtError::NotFound)?;
    if ranges.next().is_some() {
        warn!("Discarding extra vmwdt <reg> entries.");
    }

    let interrupts = node.getprop_cells(cstr!("interrupts"))?.ok_or(FdtError::NotFound)?;
    let mut chunks = CellChunkIterator::<{ WdtInfo::IRQ_CELLS }>::new(interrupts);
    let irq = chunks.next().ok_or(FdtError::NotFound)?;

    if chunks.next().is_some() {
        warn!("Discarding extra vmwdt <interrupts> entries.");
    }

    Ok(WdtInfo { addr: reg.addr, size, irq })
}

fn validate_wdt_info(wdt: &WdtInfo, num_cpus: usize) -> Result<(), RebootReason> {
    if *wdt != WdtInfo::get_expected(num_cpus) {
        error!("Invalid watchdog timer: {wdt:?}");
        return Err(RebootReason::InvalidFdt);
    }

    Ok(())
}

fn patch_wdt_info(fdt: &mut Fdt, num_cpus: usize) -> libfdt::Result<()> {
    let mut interrupts = WdtInfo::get_expected(num_cpus).irq;
    for v in interrupts.iter_mut() {
        *v = v.to_be();
    }

    let mut node = fdt
        .root_mut()
        .next_compatible(cstr!("qemu,vcpu-stall-detector"))?
        .ok_or(libfdt::FdtError::NotFound)?;
    node.setprop_inplace(cstr!("interrupts"), interrupts.as_bytes())?;
    Ok(())
}

/// Patch the DT by deleting the ns16550a compatible nodes whose address are unknown
fn patch_serial_info(fdt: &mut Fdt, serial_info: &SerialInfo) -> libfdt::Result<()> {
    let name = cstr!("ns16550a");
    let mut next = fdt.root_mut().next_compatible(name);
    while let Some(current) = next? {
        let reg =
            current.as_node().reg()?.ok_or(FdtError::NotFound)?.next().ok_or(FdtError::NotFound)?;
        next = if !serial_info.addrs.contains(&reg.addr) {
            current.delete_and_next_compatible(name)
        } else {
            current.next_compatible(name)
        }
    }
    Ok(())
}

fn validate_swiotlb_info(
    swiotlb_info: &SwiotlbInfo,
    memory: &Range<usize>,
) -> Result<(), RebootReason> {
    let size = swiotlb_info.size;
    let align = swiotlb_info.align;

    if size == 0 || (size % GUEST_PAGE_SIZE) != 0 {
        error!("Invalid swiotlb size {:#x}", size);
        return Err(RebootReason::InvalidFdt);
    }

    if let Some(align) = align.filter(|&a| a % GUEST_PAGE_SIZE != 0) {
        error!("Invalid swiotlb alignment {:#x}", align);
        return Err(RebootReason::InvalidFdt);
    }

    if let Some(addr) = swiotlb_info.addr {
        if addr.checked_add(size).is_none() {
            error!("Invalid swiotlb range: addr:{addr:#x} size:{size:#x}");
            return Err(RebootReason::InvalidFdt);
        }
    }
    if let Some(range) = swiotlb_info.fixed_range() {
        if !range.is_within(memory) {
            error!("swiotlb range {range:#x?} not part of memory range {memory:#x?}");
            return Err(RebootReason::InvalidFdt);
        }
    }

    Ok(())
}

fn patch_swiotlb_info(fdt: &mut Fdt, swiotlb_info: &SwiotlbInfo) -> libfdt::Result<()> {
    let mut node =
        fdt.root_mut().next_compatible(cstr!("restricted-dma-pool"))?.ok_or(FdtError::NotFound)?;

    if let Some(range) = swiotlb_info.fixed_range() {
        node.setprop_addrrange_inplace(
            cstr!("reg"),
            range.start.try_into().unwrap(),
            range.len().try_into().unwrap(),
        )?;
        node.nop_property(cstr!("size"))?;
        node.nop_property(cstr!("alignment"))?;
    } else {
        node.nop_property(cstr!("reg"))?;
        node.setprop_inplace(cstr!("size"), &swiotlb_info.size.to_be_bytes())?;
        node.setprop_inplace(cstr!("alignment"), &swiotlb_info.align.unwrap().to_be_bytes())?;
    }

    Ok(())
}

fn patch_gic(fdt: &mut Fdt, num_cpus: usize) -> libfdt::Result<()> {
    let node = fdt.compatible_nodes(cstr!("arm,gic-v3"))?.next().ok_or(FdtError::NotFound)?;
    let mut ranges = node.reg()?.ok_or(FdtError::NotFound)?;
    let range0 = ranges.next().ok_or(FdtError::NotFound)?;
    let mut range1 = ranges.next().ok_or(FdtError::NotFound)?;

    let addr = range0.addr;
    // `read_cpu_info_from()` guarantees that we have at most MAX_CPUS.
    const_assert!(DeviceTreeInfo::gic_patched_size(DeviceTreeInfo::MAX_CPUS).is_some());
    let size = u64::try_from(DeviceTreeInfo::gic_patched_size(num_cpus).unwrap()).unwrap();

    // range1 is just below range0
    range1.addr = addr - size;
    range1.size = Some(size);

    let (addr0, size0) = range0.to_cells();
    let (addr1, size1) = range1.to_cells();
    let value = [addr0, size0.unwrap(), addr1, size1.unwrap()];

    let mut node =
        fdt.root_mut().next_compatible(cstr!("arm,gic-v3"))?.ok_or(FdtError::NotFound)?;
    node.setprop_inplace(cstr!("reg"), flatten(&value))
}

fn patch_timer(fdt: &mut Fdt, num_cpus: usize) -> libfdt::Result<()> {
    const NUM_INTERRUPTS: usize = 4;
    const CELLS_PER_INTERRUPT: usize = 3;
    let node = fdt.compatible_nodes(cstr!("arm,armv8-timer"))?.next().ok_or(FdtError::NotFound)?;
    let interrupts = node.getprop_cells(cstr!("interrupts"))?.ok_or(FdtError::NotFound)?;
    let mut value: ArrayVec<[u32; NUM_INTERRUPTS * CELLS_PER_INTERRUPT]> =
        interrupts.take(NUM_INTERRUPTS * CELLS_PER_INTERRUPT).collect();

    let num_cpus: u32 = num_cpus.try_into().unwrap();
    // TODO(b/350498812): Rework this for >8 vCPUs.
    let cpu_mask: u32 = (((0x1 << num_cpus) - 1) & 0xff) << 8;

    for v in value.iter_mut().skip(2).step_by(CELLS_PER_INTERRUPT) {
        *v |= cpu_mask;
    }
    for v in value.iter_mut() {
        *v = v.to_be();
    }

    let value = value.into_inner();

    let mut node =
        fdt.root_mut().next_compatible(cstr!("arm,armv8-timer"))?.ok_or(FdtError::NotFound)?;
    node.setprop_inplace(cstr!("interrupts"), value.as_bytes())
}

fn patch_untrusted_props(fdt: &mut Fdt, props: &BTreeMap<CString, Vec<u8>>) -> libfdt::Result<()> {
    let avf_node = if let Some(node) = fdt.node_mut(cstr!("/avf"))? {
        node
    } else {
        fdt.root_mut().add_subnode(cstr!("avf"))?
    };

    // The node shouldn't already be present; if it is, return the error.
    let mut node = avf_node.add_subnode(cstr!("untrusted"))?;

    for (name, value) in props {
        node.setprop(name, value)?;
    }

    Ok(())
}

#[derive(Debug)]
struct VcpufreqInfo {
    addr: u64,
    size: u64,
}

fn patch_vcpufreq(fdt: &mut Fdt, vcpufreq_info: &Option<VcpufreqInfo>) -> libfdt::Result<()> {
    let mut node = fdt.node_mut(cstr!("/cpufreq"))?.unwrap();
    if let Some(info) = vcpufreq_info {
        node.setprop_addrrange_inplace(cstr!("reg"), info.addr, info.size)
    } else {
        node.nop()
    }
}

#[derive(Debug)]
pub struct DeviceTreeInfo {
    pub kernel_range: Option<Range<usize>>,
    pub initrd_range: Option<Range<usize>>,
    pub memory_range: Range<usize>,
    bootargs: Option<CString>,
    cpus: ArrayVec<[CpuInfo; DeviceTreeInfo::MAX_CPUS]>,
    cpu_topology: Option<CpuTopology>,
    pci_info: PciInfo,
    serial_info: SerialInfo,
    pub swiotlb_info: SwiotlbInfo,
    device_assignment: Option<DeviceAssignmentInfo>,
    untrusted_props: BTreeMap<CString, Vec<u8>>,
    vm_ref_dt_props_info: BTreeMap<CString, Vec<u8>>,
    vcpufreq_info: Option<VcpufreqInfo>,
}

impl DeviceTreeInfo {
    const MAX_CPUS: usize = 16;

    const fn gic_patched_size(num_cpus: usize) -> Option<usize> {
        const GIC_REDIST_SIZE_PER_CPU: usize = 32 * SIZE_4KB;

        GIC_REDIST_SIZE_PER_CPU.checked_mul(num_cpus)
    }
}

pub fn sanitize_device_tree(
    fdt: &mut [u8],
    vm_dtbo: Option<&mut [u8]>,
    vm_ref_dt: Option<&[u8]>,
) -> Result<DeviceTreeInfo, RebootReason> {
    let fdt = Fdt::from_mut_slice(fdt).map_err(|e| {
        error!("Failed to load FDT: {e}");
        RebootReason::InvalidFdt
    })?;

    let vm_dtbo = match vm_dtbo {
        Some(vm_dtbo) => Some(VmDtbo::from_mut_slice(vm_dtbo).map_err(|e| {
            error!("Failed to load VM DTBO: {e}");
            RebootReason::InvalidFdt
        })?),
        None => None,
    };

    let info = parse_device_tree(fdt, vm_dtbo.as_deref())?;

    // SAFETY: We trust that the template (hardcoded in our RO data) is a valid DT.
    let fdt_template = unsafe { Fdt::unchecked_from_slice(pvmfw_fdt_template::RAW) };
    fdt.clone_from(fdt_template).map_err(|e| {
        error!("Failed to instantiate FDT from the template DT: {e}");
        RebootReason::InvalidFdt
    })?;

    fdt.unpack().map_err(|e| {
        error!("Failed to unpack DT for patching: {e}");
        RebootReason::InvalidFdt
    })?;

    if let Some(device_assignment_info) = &info.device_assignment {
        let vm_dtbo = vm_dtbo.unwrap();
        device_assignment_info.filter(vm_dtbo).map_err(|e| {
            error!("Failed to filter VM DTBO: {e}");
            RebootReason::InvalidFdt
        })?;
        // SAFETY: Damaged VM DTBO isn't used in this API after this unsafe block.
        // VM DTBO can't be reused in any way as Fdt nor VmDtbo outside of this API because
        // it can only be instantiated after validation.
        unsafe {
            fdt.apply_overlay(vm_dtbo.as_mut()).map_err(|e| {
                error!("Failed to apply filtered VM DTBO: {e}");
                RebootReason::InvalidFdt
            })?;
        }
    }

    if let Some(vm_ref_dt) = vm_ref_dt {
        let vm_ref_dt = Fdt::from_slice(vm_ref_dt).map_err(|e| {
            error!("Failed to load VM reference DT: {e}");
            RebootReason::InvalidFdt
        })?;

        validate_vm_ref_dt(fdt, vm_ref_dt, &info.vm_ref_dt_props_info).map_err(|e| {
            error!("Failed to apply VM reference DT: {e}");
            RebootReason::InvalidFdt
        })?;
    }

    patch_device_tree(fdt, &info)?;

    // TODO(b/317201360): Ensure no overlapping in <reg> among devices

    fdt.pack().map_err(|e| {
        error!("Failed to unpack DT after patching: {e}");
        RebootReason::InvalidFdt
    })?;

    Ok(info)
}

fn parse_device_tree(fdt: &Fdt, vm_dtbo: Option<&VmDtbo>) -> Result<DeviceTreeInfo, RebootReason> {
    let kernel_range = read_kernel_range_from(fdt).map_err(|e| {
        error!("Failed to read kernel range from DT: {e}");
        RebootReason::InvalidFdt
    })?;

    let initrd_range = read_initrd_range_from(fdt).map_err(|e| {
        error!("Failed to read initrd range from DT: {e}");
        RebootReason::InvalidFdt
    })?;

    let memory_range = read_and_validate_memory_range(fdt)?;

    let bootargs = read_bootargs_from(fdt).map_err(|e| {
        error!("Failed to read bootargs from DT: {e}");
        RebootReason::InvalidFdt
    })?;

    let (cpus, cpu_topology) = read_cpu_info_from(fdt).map_err(|e| {
        error!("Failed to read CPU info from DT: {e}");
        RebootReason::InvalidFdt
    })?;
    validate_cpu_info(&cpus).map_err(|e| {
        error!("Failed to validate CPU info from DT: {e}");
        RebootReason::InvalidFdt
    })?;

    let vcpufreq_info = read_vcpufreq_info(fdt).map_err(|e| {
        error!("Failed to read vcpufreq info from DT: {e}");
        RebootReason::InvalidFdt
    })?;
    if let Some(ref info) = vcpufreq_info {
        validate_vcpufreq_info(info, &cpus).map_err(|e| {
            error!("Failed to validate vcpufreq info from DT: {e}");
            RebootReason::InvalidFdt
        })?;
    }

    let pci_info = read_pci_info_from(fdt).map_err(|e| {
        error!("Failed to read pci info from DT: {e}");
        RebootReason::InvalidFdt
    })?;
    validate_pci_info(&pci_info, &memory_range)?;

    let wdt_info = read_wdt_info_from(fdt).map_err(|e| {
        error!("Failed to read vCPU stall detector info from DT: {e}");
        RebootReason::InvalidFdt
    })?;
    validate_wdt_info(&wdt_info, cpus.len())?;

    let serial_info = read_serial_info_from(fdt).map_err(|e| {
        error!("Failed to read serial info from DT: {e}");
        RebootReason::InvalidFdt
    })?;

    let swiotlb_info = SwiotlbInfo::new_from_fdt(fdt).map_err(|e| {
        error!("Failed to read swiotlb info from DT: {e}");
        RebootReason::InvalidFdt
    })?;
    validate_swiotlb_info(&swiotlb_info, &memory_range)?;

    let device_assignment = match vm_dtbo {
        Some(vm_dtbo) => {
            if let Some(hypervisor) = hyp::get_device_assigner() {
                DeviceAssignmentInfo::parse(fdt, vm_dtbo, hypervisor).map_err(|e| {
                    error!("Failed to parse device assignment from DT and VM DTBO: {e}");
                    RebootReason::InvalidFdt
                })?
            } else {
                warn!(
                    "Device assignment is ignored because device assigning hypervisor is missing"
                );
                None
            }
        }
        None => None,
    };

    let untrusted_props = parse_untrusted_props(fdt).map_err(|e| {
        error!("Failed to read untrusted properties: {e}");
        RebootReason::InvalidFdt
    })?;
    validate_untrusted_props(&untrusted_props).map_err(|e| {
        error!("Failed to validate untrusted properties: {e}");
        RebootReason::InvalidFdt
    })?;

    let vm_ref_dt_props_info = parse_vm_ref_dt(fdt).map_err(|e| {
        error!("Failed to read names of properties under /avf from DT: {e}");
        RebootReason::InvalidFdt
    })?;

    Ok(DeviceTreeInfo {
        kernel_range,
        initrd_range,
        memory_range,
        bootargs,
        cpus,
        cpu_topology,
        pci_info,
        serial_info,
        swiotlb_info,
        device_assignment,
        untrusted_props,
        vm_ref_dt_props_info,
        vcpufreq_info,
    })
}

fn patch_device_tree(fdt: &mut Fdt, info: &DeviceTreeInfo) -> Result<(), RebootReason> {
    if let Some(initrd_range) = &info.initrd_range {
        patch_initrd_range(fdt, initrd_range).map_err(|e| {
            error!("Failed to patch initrd range to DT: {e}");
            RebootReason::InvalidFdt
        })?;
    }
    patch_memory_range(fdt, &info.memory_range).map_err(|e| {
        error!("Failed to patch memory range to DT: {e}");
        RebootReason::InvalidFdt
    })?;
    if let Some(bootargs) = &info.bootargs {
        patch_bootargs(fdt, bootargs.as_c_str()).map_err(|e| {
            error!("Failed to patch bootargs to DT: {e}");
            RebootReason::InvalidFdt
        })?;
    }
    patch_cpus(fdt, &info.cpus, &info.cpu_topology).map_err(|e| {
        error!("Failed to patch cpus to DT: {e}");
        RebootReason::InvalidFdt
    })?;
    patch_vcpufreq(fdt, &info.vcpufreq_info).map_err(|e| {
        error!("Failed to patch vcpufreq info to DT: {e}");
        RebootReason::InvalidFdt
    })?;
    patch_pci_info(fdt, &info.pci_info).map_err(|e| {
        error!("Failed to patch pci info to DT: {e}");
        RebootReason::InvalidFdt
    })?;
    patch_wdt_info(fdt, info.cpus.len()).map_err(|e| {
        error!("Failed to patch wdt info to DT: {e}");
        RebootReason::InvalidFdt
    })?;
    patch_serial_info(fdt, &info.serial_info).map_err(|e| {
        error!("Failed to patch serial info to DT: {e}");
        RebootReason::InvalidFdt
    })?;
    patch_swiotlb_info(fdt, &info.swiotlb_info).map_err(|e| {
        error!("Failed to patch swiotlb info to DT: {e}");
        RebootReason::InvalidFdt
    })?;
    patch_gic(fdt, info.cpus.len()).map_err(|e| {
        error!("Failed to patch gic info to DT: {e}");
        RebootReason::InvalidFdt
    })?;
    patch_timer(fdt, info.cpus.len()).map_err(|e| {
        error!("Failed to patch timer info to DT: {e}");
        RebootReason::InvalidFdt
    })?;
    if let Some(device_assignment) = &info.device_assignment {
        // Note: We patch values after VM DTBO is overlaid because patch may require more space
        // then VM DTBO's underlying slice is allocated.
        device_assignment.patch(fdt).map_err(|e| {
            error!("Failed to patch device assignment info to DT: {e}");
            RebootReason::InvalidFdt
        })?;
    } else {
        device_assignment::clean(fdt).map_err(|e| {
            error!("Failed to clean pre-polulated DT nodes for device assignment: {e}");
            RebootReason::InvalidFdt
        })?;
    }
    patch_untrusted_props(fdt, &info.untrusted_props).map_err(|e| {
        error!("Failed to patch untrusted properties: {e}");
        RebootReason::InvalidFdt
    })?;

    Ok(())
}

/// Modifies the input DT according to the fields of the configuration.
pub fn modify_for_next_stage(
    fdt: &mut Fdt,
    bcc: &[u8],
    new_instance: bool,
    strict_boot: bool,
    debug_policy: Option<&[u8]>,
    debuggable: bool,
    kaslr_seed: u64,
) -> libfdt::Result<()> {
    if let Some(debug_policy) = debug_policy {
        let backup = Vec::from(fdt.as_slice());
        fdt.unpack()?;
        let backup_fdt = Fdt::from_slice(backup.as_slice()).unwrap();
        if apply_debug_policy(fdt, backup_fdt, debug_policy)? {
            info!("Debug policy applied.");
        } else {
            // apply_debug_policy restored fdt to backup_fdt so unpack it again.
            fdt.unpack()?;
        }
    } else {
        info!("No debug policy found.");
        fdt.unpack()?;
    }

    patch_dice_node(fdt, bcc.as_ptr() as usize, bcc.len())?;

    if let Some(mut chosen) = fdt.chosen_mut()? {
        empty_or_delete_prop(&mut chosen, cstr!("avf,strict-boot"), strict_boot)?;
        empty_or_delete_prop(&mut chosen, cstr!("avf,new-instance"), new_instance)?;
        chosen.setprop_inplace(cstr!("kaslr-seed"), &kaslr_seed.to_be_bytes())?;
    };
    if !debuggable {
        if let Some(bootargs) = read_bootargs_from(fdt)? {
            filter_out_dangerous_bootargs(fdt, &bootargs)?;
        }
    }

    fdt.pack()?;

    Ok(())
}

/// Patch the "google,open-dice"-compatible reserved-memory node to point to the bcc range
fn patch_dice_node(fdt: &mut Fdt, addr: usize, size: usize) -> libfdt::Result<()> {
    // We reject DTs with missing reserved-memory node as validation should have checked that the
    // "swiotlb" subnode (compatible = "restricted-dma-pool") was present.
    let node = fdt.node_mut(cstr!("/reserved-memory"))?.ok_or(libfdt::FdtError::NotFound)?;

    let mut node = node.next_compatible(cstr!("google,open-dice"))?.ok_or(FdtError::NotFound)?;

    let addr: u64 = addr.try_into().unwrap();
    let size: u64 = size.try_into().unwrap();
    node.setprop_inplace(cstr!("reg"), flatten(&[addr.to_be_bytes(), size.to_be_bytes()]))
}

fn empty_or_delete_prop(
    fdt_node: &mut FdtNodeMut,
    prop_name: &CStr,
    keep_prop: bool,
) -> libfdt::Result<()> {
    if keep_prop {
        fdt_node.setprop_empty(prop_name)
    } else {
        fdt_node
            .delprop(prop_name)
            .or_else(|e| if e == FdtError::NotFound { Ok(()) } else { Err(e) })
    }
}

/// Apply the debug policy overlay to the guest DT.
///
/// Returns Ok(true) on success, Ok(false) on recovered failure and Err(_) on corruption of the DT.
fn apply_debug_policy(
    fdt: &mut Fdt,
    backup_fdt: &Fdt,
    debug_policy: &[u8],
) -> libfdt::Result<bool> {
    let mut debug_policy = Vec::from(debug_policy);
    let overlay = match Fdt::from_mut_slice(debug_policy.as_mut_slice()) {
        Ok(overlay) => overlay,
        Err(e) => {
            warn!("Corrupted debug policy found: {e}. Not applying.");
            return Ok(false);
        }
    };

    // SAFETY: on failure, the corrupted DT is restored using the backup.
    if let Err(e) = unsafe { fdt.apply_overlay(overlay) } {
        warn!("Failed to apply debug policy: {e}. Recovering...");
        fdt.clone_from(backup_fdt)?;
        // A successful restoration is considered success because an invalid debug policy
        // shouldn't DOS the pvmfw
        Ok(false)
    } else {
        Ok(true)
    }
}

fn has_common_debug_policy(fdt: &Fdt, debug_feature_name: &CStr) -> libfdt::Result<bool> {
    if let Some(node) = fdt.node(cstr!("/avf/guest/common"))? {
        if let Some(value) = node.getprop_u32(debug_feature_name)? {
            return Ok(value == 1);
        }
    }
    Ok(false) // if the policy doesn't exist or not 1, don't enable the debug feature
}

fn filter_out_dangerous_bootargs(fdt: &mut Fdt, bootargs: &CStr) -> libfdt::Result<()> {
    let has_crashkernel = has_common_debug_policy(fdt, cstr!("ramdump"))?;
    let has_console = has_common_debug_policy(fdt, cstr!("log"))?;

    let accepted: &[(&str, Box<dyn Fn(Option<&str>) -> bool>)] = &[
        ("panic", Box::new(|v| if let Some(v) = v { v == "=-1" } else { false })),
        ("crashkernel", Box::new(|_| has_crashkernel)),
        ("console", Box::new(|_| has_console)),
    ];

    // parse and filter out unwanted
    let mut filtered = Vec::new();
    for arg in BootArgsIterator::new(bootargs).map_err(|e| {
        info!("Invalid bootarg: {e}");
        FdtError::BadValue
    })? {
        match accepted.iter().find(|&t| t.0 == arg.name()) {
            Some((_, pred)) if pred(arg.value()) => filtered.push(arg),
            _ => debug!("Rejected bootarg {}", arg.as_ref()),
        }
    }

    // flatten into a new C-string
    let mut new_bootargs = Vec::new();
    for (i, arg) in filtered.iter().enumerate() {
        if i != 0 {
            new_bootargs.push(b' '); // separator
        }
        new_bootargs.extend_from_slice(arg.as_ref().as_bytes());
    }
    new_bootargs.push(b'\0');

    let mut node = fdt.chosen_mut()?.ok_or(FdtError::NotFound)?;
    node.setprop(cstr!("bootargs"), new_bootargs.as_slice())
}
