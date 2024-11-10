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

//! Wrapper around libfdt library. Provides parsing/generating functionality
//! to a bare-metal environment.

#![no_std]

mod iterators;
mod libfdt;
mod result;
mod safe_types;

pub use iterators::{
    AddressRange, CellIterator, CompatibleIterator, DescendantsIterator, MemRegIterator,
    PropertyIterator, RangesIterator, Reg, RegIterator, SubnodeIterator,
};
pub use result::{FdtError, Result};
pub use safe_types::{FdtHeader, NodeOffset, Phandle, PropOffset, StringOffset};

use core::ffi::{c_void, CStr};
use core::ops::Range;
use cstr::cstr;
use libfdt::get_slice_at_ptr;
use zerocopy::AsBytes as _;

use crate::libfdt::{Libfdt, LibfdtMut};

/// Value of a #address-cells property.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum AddrCells {
    Single = 1,
    Double = 2,
    Triple = 3,
}

impl TryFrom<usize> for AddrCells {
    type Error = FdtError;

    fn try_from(value: usize) -> Result<Self> {
        match value {
            x if x == Self::Single as _ => Ok(Self::Single),
            x if x == Self::Double as _ => Ok(Self::Double),
            x if x == Self::Triple as _ => Ok(Self::Triple),
            _ => Err(FdtError::BadNCells),
        }
    }
}

/// Value of a #size-cells property.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SizeCells {
    None = 0,
    Single = 1,
    Double = 2,
}

impl TryFrom<usize> for SizeCells {
    type Error = FdtError;

    fn try_from(value: usize) -> Result<Self> {
        match value {
            x if x == Self::None as _ => Ok(Self::None),
            x if x == Self::Single as _ => Ok(Self::Single),
            x if x == Self::Double as _ => Ok(Self::Double),
            _ => Err(FdtError::BadNCells),
        }
    }
}

/// DT property wrapper to abstract endianess changes
#[repr(transparent)]
#[derive(Debug)]
struct FdtPropertyStruct(libfdt_bindgen::fdt_property);

impl AsRef<FdtPropertyStruct> for libfdt_bindgen::fdt_property {
    fn as_ref(&self) -> &FdtPropertyStruct {
        let ptr = self as *const _ as *const _;
        // SAFETY: Types have the same layout (transparent) so the valid reference remains valid.
        unsafe { &*ptr }
    }
}

impl FdtPropertyStruct {
    fn from_offset(fdt: &Fdt, offset: PropOffset) -> Result<&Self> {
        Ok(fdt.get_property_by_offset(offset)?.as_ref())
    }

    fn name_offset(&self) -> StringOffset {
        StringOffset(u32::from_be(self.0.nameoff).try_into().unwrap())
    }

    fn data_len(&self) -> usize {
        u32::from_be(self.0.len).try_into().unwrap()
    }

    fn data_ptr(&self) -> *const c_void {
        self.0.data.as_ptr().cast()
    }
}

/// DT property.
#[derive(Clone, Copy, Debug)]
pub struct FdtProperty<'a> {
    fdt: &'a Fdt,
    offset: PropOffset,
    property: &'a FdtPropertyStruct,
}

impl<'a> FdtProperty<'a> {
    fn new(fdt: &'a Fdt, offset: PropOffset) -> Result<Self> {
        let property = FdtPropertyStruct::from_offset(fdt, offset)?;
        Ok(Self { fdt, offset, property })
    }

    /// Returns the property name
    pub fn name(&self) -> Result<&'a CStr> {
        self.fdt.string(self.property.name_offset())
    }

    /// Returns the property value
    pub fn value(&self) -> Result<&'a [u8]> {
        self.fdt.get_from_ptr(self.property.data_ptr(), self.property.data_len())
    }

    fn next_property(&self) -> Result<Option<Self>> {
        if let Some(offset) = self.fdt.next_property_offset(self.offset)? {
            Ok(Some(Self::new(self.fdt, offset)?))
        } else {
            Ok(None)
        }
    }
}

/// DT node.
#[derive(Clone, Copy, Debug)]
pub struct FdtNode<'a> {
    fdt: &'a Fdt,
    offset: NodeOffset,
}

impl<'a> FdtNode<'a> {
    /// Returns parent node.
    pub fn parent(&self) -> Result<Self> {
        let offset = self.fdt.parent_offset(self.offset)?;

        Ok(Self { fdt: self.fdt, offset })
    }

    /// Returns supernode with depth. Note that root is at depth 0.
    pub fn supernode_at_depth(&self, depth: usize) -> Result<Self> {
        let offset = self.fdt.supernode_atdepth_offset(self.offset, depth)?;

        Ok(Self { fdt: self.fdt, offset })
    }

    /// Returns the standard (deprecated) device_type <string> property.
    pub fn device_type(&self) -> Result<Option<&CStr>> {
        self.getprop_str(cstr!("device_type"))
    }

    /// Returns the standard reg <prop-encoded-array> property.
    pub fn reg(&self) -> Result<Option<RegIterator<'a>>> {
        if let Some(cells) = self.getprop_cells(cstr!("reg"))? {
            let parent = self.parent()?;

            let addr_cells = parent.address_cells()?;
            let size_cells = parent.size_cells()?;

            Ok(Some(RegIterator::new(cells, addr_cells, size_cells)))
        } else {
            Ok(None)
        }
    }

    /// Returns the standard ranges property.
    pub fn ranges<A, P, S>(&self) -> Result<Option<RangesIterator<'a, A, P, S>>> {
        if let Some(cells) = self.getprop_cells(cstr!("ranges"))? {
            let parent = self.parent()?;
            let addr_cells = self.address_cells()?;
            let parent_addr_cells = parent.address_cells()?;
            let size_cells = self.size_cells()?;
            Ok(Some(RangesIterator::<A, P, S>::new(
                cells,
                addr_cells,
                parent_addr_cells,
                size_cells,
            )))
        } else {
            Ok(None)
        }
    }

    /// Returns the node name.
    pub fn name(&self) -> Result<&'a CStr> {
        let name = self.fdt.get_name(self.offset)?;
        CStr::from_bytes_with_nul(name).map_err(|_| FdtError::Internal)
    }

    /// Returns the value of a given <string> property.
    pub fn getprop_str(&self, name: &CStr) -> Result<Option<&CStr>> {
        if let Some(bytes) = self.getprop(name)? {
            Ok(Some(CStr::from_bytes_with_nul(bytes).map_err(|_| FdtError::BadValue)?))
        } else {
            Ok(None)
        }
    }

    /// Returns the value of a given property as an array of cells.
    pub fn getprop_cells(&self, name: &CStr) -> Result<Option<CellIterator<'a>>> {
        if let Some(cells) = self.getprop(name)? {
            Ok(Some(CellIterator::new(cells)))
        } else {
            Ok(None)
        }
    }

    /// Returns the value of a given <u32> property.
    pub fn getprop_u32(&self, name: &CStr) -> Result<Option<u32>> {
        if let Some(bytes) = self.getprop(name)? {
            Ok(Some(u32::from_be_bytes(bytes.try_into().map_err(|_| FdtError::BadValue)?)))
        } else {
            Ok(None)
        }
    }

    /// Returns the value of a given <u64> property.
    pub fn getprop_u64(&self, name: &CStr) -> Result<Option<u64>> {
        if let Some(bytes) = self.getprop(name)? {
            Ok(Some(u64::from_be_bytes(bytes.try_into().map_err(|_| FdtError::BadValue)?)))
        } else {
            Ok(None)
        }
    }

    /// Returns the value of a given property.
    pub fn getprop(&self, name: &CStr) -> Result<Option<&'a [u8]>> {
        self.fdt.getprop_namelen(self.offset, name.to_bytes())
    }

    /// Returns reference to the containing device tree.
    pub fn fdt(&self) -> &Fdt {
        self.fdt
    }

    /// Returns the compatible node of the given name that is next after this node.
    pub fn next_compatible(self, compatible: &CStr) -> Result<Option<Self>> {
        let offset = self.fdt.node_offset_by_compatible(self.offset, compatible)?;

        Ok(offset.map(|offset| Self { fdt: self.fdt, offset }))
    }

    /// Returns the first range of `reg` in this node.
    pub fn first_reg(&self) -> Result<Reg<u64>> {
        self.reg()?.ok_or(FdtError::NotFound)?.next().ok_or(FdtError::NotFound)
    }

    fn address_cells(&self) -> Result<AddrCells> {
        self.fdt.address_cells(self.offset)?.try_into()
    }

    fn size_cells(&self) -> Result<SizeCells> {
        self.fdt.size_cells(self.offset)?.try_into()
    }

    /// Returns an iterator of subnodes
    pub fn subnodes(&self) -> Result<SubnodeIterator<'a>> {
        SubnodeIterator::new(self)
    }

    fn first_subnode(&self) -> Result<Option<Self>> {
        let offset = self.fdt.first_subnode(self.offset)?;

        Ok(offset.map(|offset| Self { fdt: self.fdt, offset }))
    }

    fn next_subnode(&self) -> Result<Option<Self>> {
        let offset = self.fdt.next_subnode(self.offset)?;

        Ok(offset.map(|offset| Self { fdt: self.fdt, offset }))
    }

    /// Returns an iterator of descendants
    pub fn descendants(&self) -> DescendantsIterator<'a> {
        DescendantsIterator::new(self)
    }

    fn next_node(&self, depth: usize) -> Result<Option<(Self, usize)>> {
        if let Some((offset, depth)) = self.fdt.next_node(self.offset, depth)? {
            Ok(Some((Self { fdt: self.fdt, offset }, depth)))
        } else {
            Ok(None)
        }
    }

    /// Returns an iterator of properties
    pub fn properties(&'a self) -> Result<PropertyIterator<'a>> {
        PropertyIterator::new(self)
    }

    fn first_property(&self) -> Result<Option<FdtProperty<'a>>> {
        if let Some(offset) = self.fdt.first_property_offset(self.offset)? {
            Ok(Some(FdtProperty::new(self.fdt, offset)?))
        } else {
            Ok(None)
        }
    }

    /// Returns the phandle
    pub fn get_phandle(&self) -> Result<Option<Phandle>> {
        // This rewrites the fdt_get_phandle() because it doesn't return error code.
        if let Some(prop) = self.getprop_u32(cstr!("phandle"))? {
            Ok(Some(prop.try_into()?))
        } else if let Some(prop) = self.getprop_u32(cstr!("linux,phandle"))? {
            Ok(Some(prop.try_into()?))
        } else {
            Ok(None)
        }
    }

    /// Returns the subnode of the given name. The name doesn't need to be nul-terminated.
    pub fn subnode(&self, name: &CStr) -> Result<Option<Self>> {
        let name = name.to_bytes();
        let offset = self.fdt.subnode_offset_namelen(self.offset, name)?;

        Ok(offset.map(|offset| Self { fdt: self.fdt, offset }))
    }

    /// Returns the subnode of the given name bytes
    pub fn subnode_with_name_bytes(&self, name: &[u8]) -> Result<Option<Self>> {
        let offset = self.fdt.subnode_offset_namelen(self.offset, name)?;

        Ok(offset.map(|offset| Self { fdt: self.fdt, offset }))
    }
}

impl<'a> PartialEq for FdtNode<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.fdt.as_ptr() == other.fdt.as_ptr() && self.offset == other.offset
    }
}

/// Mutable FDT node.
#[derive(Debug)]
pub struct FdtNodeMut<'a> {
    fdt: &'a mut Fdt,
    offset: NodeOffset,
}

impl<'a> FdtNodeMut<'a> {
    /// Appends a property name-value (possibly empty) pair to the given node.
    pub fn appendprop<T: AsRef<[u8]>>(&mut self, name: &CStr, value: &T) -> Result<()> {
        self.fdt.appendprop(self.offset, name, value.as_ref())
    }

    /// Appends a (address, size) pair property to the given node.
    pub fn appendprop_addrrange(&mut self, name: &CStr, addr: u64, size: u64) -> Result<()> {
        let parent = self.parent()?.offset;
        self.fdt.appendprop_addrrange(parent, self.offset, name, addr, size)
    }

    /// Sets a property name-value pair to the given node.
    ///
    /// This may create a new prop or replace existing value.
    pub fn setprop(&mut self, name: &CStr, value: &[u8]) -> Result<()> {
        self.fdt.setprop(self.offset, name, value)
    }

    /// Sets the value of the given property with the given value, and ensure that the given
    /// value has the same length as the current value length.
    ///
    /// This can only be used to replace existing value.
    pub fn setprop_inplace(&mut self, name: &CStr, value: &[u8]) -> Result<()> {
        self.fdt.setprop_inplace(self.offset, name, value)
    }

    /// Sets the value of the given (address, size) pair property with the given value, and
    /// ensure that the given value has the same length as the current value length.
    ///
    /// This can only be used to replace existing value.
    pub fn setprop_addrrange_inplace(&mut self, name: &CStr, addr: u64, size: u64) -> Result<()> {
        let pair = [addr.to_be(), size.to_be()];
        self.fdt.setprop_inplace(self.offset, name, pair.as_bytes())
    }

    /// Sets a flag-like empty property.
    ///
    /// This may create a new prop or replace existing value.
    pub fn setprop_empty(&mut self, name: &CStr) -> Result<()> {
        self.fdt.setprop(self.offset, name, &[])
    }

    /// Deletes the given property.
    pub fn delprop(&mut self, name: &CStr) -> Result<()> {
        self.fdt.delprop(self.offset, name)
    }

    /// Deletes the given property effectively from DT, by setting it with FDT_NOP.
    pub fn nop_property(&mut self, name: &CStr) -> Result<()> {
        self.fdt.nop_property(self.offset, name)
    }

    /// Trims the size of the given property to new_size.
    pub fn trimprop(&mut self, name: &CStr, new_size: usize) -> Result<()> {
        let prop = self.as_node().getprop(name)?.ok_or(FdtError::NotFound)?;

        match prop.len() {
            x if x == new_size => Ok(()),
            x if x < new_size => Err(FdtError::NoSpace),
            _ => self.fdt.setprop_placeholder(self.offset, name, new_size).map(|_| ()),
        }
    }

    /// Returns reference to the containing device tree.
    pub fn fdt(&mut self) -> &mut Fdt {
        self.fdt
    }

    /// Returns immutable FdtNode of this node.
    pub fn as_node(&self) -> FdtNode {
        FdtNode { fdt: self.fdt, offset: self.offset }
    }

    /// Adds new subnodes to the given node.
    pub fn add_subnodes(self, names: &[&CStr]) -> Result<()> {
        for name in names {
            self.fdt.add_subnode_namelen(self.offset, name.to_bytes())?;
        }
        Ok(())
    }

    /// Adds a new subnode to the given node and return it as a FdtNodeMut on success.
    pub fn add_subnode(self, name: &CStr) -> Result<Self> {
        let name = name.to_bytes();
        let offset = self.fdt.add_subnode_namelen(self.offset, name)?;

        Ok(Self { fdt: self.fdt, offset })
    }

    /// Adds a new subnode to the given node with name and namelen, and returns it as a FdtNodeMut
    /// on success.
    pub fn add_subnode_with_namelen(self, name: &CStr, namelen: usize) -> Result<Self> {
        let name = &name.to_bytes()[..namelen];
        let offset = self.fdt.add_subnode_namelen(self.offset, name)?;

        Ok(Self { fdt: self.fdt, offset })
    }

    /// Returns the first subnode of this
    pub fn first_subnode(self) -> Result<Option<Self>> {
        let offset = self.fdt.first_subnode(self.offset)?;

        Ok(offset.map(|offset| Self { fdt: self.fdt, offset }))
    }

    /// Returns the next subnode that shares the same parent with this
    pub fn next_subnode(self) -> Result<Option<Self>> {
        let offset = self.fdt.next_subnode(self.offset)?;

        Ok(offset.map(|offset| Self { fdt: self.fdt, offset }))
    }

    /// Deletes the current node and returns the next subnode
    pub fn delete_and_next_subnode(self) -> Result<Option<Self>> {
        let next_offset = self.fdt.next_subnode(self.offset)?;

        self.delete_and_next(next_offset)
    }

    /// Returns the next node. Use this API to travel descendant of a node.
    ///
    /// Returned depth is relative to the initial node that had called with any of next node APIs.
    /// Returns None if end of FDT reached or depth becomes negative.
    ///
    /// See also: [`next_node_skip_subnodes`], and [`delete_and_next_node`]
    pub fn next_node(self, depth: usize) -> Result<Option<(Self, usize)>> {
        let next = self.fdt.next_node(self.offset, depth)?;

        Ok(next.map(|(offset, depth)| (Self { fdt: self.fdt, offset }, depth)))
    }

    /// Returns the next node skipping subnodes. Use this API to travel descendants of a node while
    /// ignoring certain node.
    ///
    /// Returned depth is relative to the initial node that had called with any of next node APIs.
    /// Returns None if end of FDT reached or depth becomes negative.
    ///
    /// See also: [`next_node`], and [`delete_and_next_node`]
    pub fn next_node_skip_subnodes(self, depth: usize) -> Result<Option<(Self, usize)>> {
        let next = self.fdt.next_node_skip_subnodes(self.offset, depth)?;

        Ok(next.map(|(offset, depth)| (Self { fdt: self.fdt, offset }, depth)))
    }

    /// Deletes this and returns the next node. Use this API to travel descendants of a node while
    /// removing certain node.
    ///
    /// Returned depth is relative to the initial node that had called with any of next node APIs.
    /// Returns None if end of FDT reached or depth becomes negative.
    ///
    /// See also: [`next_node`], and [`next_node_skip_subnodes`]
    pub fn delete_and_next_node(self, depth: usize) -> Result<Option<(Self, usize)>> {
        let next_node = self.fdt.next_node_skip_subnodes(self.offset, depth)?;
        if let Some((offset, depth)) = next_node {
            let next_node = self.delete_and_next(Some(offset))?.unwrap();
            Ok(Some((next_node, depth)))
        } else {
            self.delete_and_next(None)?;
            Ok(None)
        }
    }

    fn parent(&'a self) -> Result<FdtNode<'a>> {
        self.as_node().parent()
    }

    /// Returns the compatible node of the given name that is next after this node.
    pub fn next_compatible(self, compatible: &CStr) -> Result<Option<Self>> {
        let offset = self.fdt.node_offset_by_compatible(self.offset, compatible)?;

        Ok(offset.map(|offset| Self { fdt: self.fdt, offset }))
    }

    /// Deletes the node effectively by overwriting this node and its subtree with nop tags.
    /// Returns the next compatible node of the given name.
    // Side note: without this, filterint out excessive compatible nodes from the DT is impossible.
    // The reason is that libfdt ensures that the node from where the search for the next
    // compatible node is started is always a valid one -- except for the special case of offset =
    // -1 which is to find the first compatible node. So, we can't delete a node and then find the
    // next compatible node from it.
    //
    // We can't do in the opposite direction either. If we call next_compatible to find the next
    // node, and delete the current node, the Rust borrow checker kicks in. The next node has a
    // mutable reference to DT, so we can't use current node (which also has a mutable reference to
    // DT).
    pub fn delete_and_next_compatible(self, compatible: &CStr) -> Result<Option<Self>> {
        let next_offset = self.fdt.node_offset_by_compatible(self.offset, compatible)?;

        self.delete_and_next(next_offset)
    }

    fn delete_and_next(self, next_offset: Option<NodeOffset>) -> Result<Option<Self>> {
        if Some(self.offset) == next_offset {
            return Err(FdtError::Internal);
        }

        self.fdt.nop_node(self.offset)?;

        Ok(next_offset.map(|offset| Self { fdt: self.fdt, offset }))
    }

    /// Deletes this node effectively from DT, by setting it with FDT_NOP
    pub fn nop(self) -> Result<()> {
        self.fdt.nop_node(self.offset)
    }
}

/// Wrapper around low-level libfdt functions.
#[derive(Debug)]
#[repr(transparent)]
pub struct Fdt {
    buffer: [u8],
}

// SAFETY: Fdt calls check_full() before safely returning a &Self, making it impossible for trait
// methods to be called on invalid device trees.
unsafe impl Libfdt for Fdt {
    fn as_fdt_slice(&self) -> &[u8] {
        &self.buffer[..self.totalsize()]
    }
}

// SAFETY: Fdt calls check_full() before safely returning a &Self, making it impossible for trait
// methods to be called on invalid device trees.
unsafe impl LibfdtMut for Fdt {
    fn as_fdt_slice_mut(&mut self) -> &mut [u8] {
        &mut self.buffer
    }
}

impl Fdt {
    /// Wraps a slice containing a Flattened Device Tree.
    ///
    /// Fails if the FDT does not pass validation.
    pub fn from_slice(fdt: &[u8]) -> Result<&Self> {
        libfdt::check_full(fdt)?;
        // SAFETY: The FDT was validated.
        let fdt = unsafe { Self::unchecked_from_slice(fdt) };

        Ok(fdt)
    }

    /// Wraps a mutable slice containing a Flattened Device Tree.
    ///
    /// Fails if the FDT does not pass validation.
    pub fn from_mut_slice(fdt: &mut [u8]) -> Result<&mut Self> {
        libfdt::check_full(fdt)?;
        // SAFETY: The FDT was validated.
        let fdt = unsafe { Self::unchecked_from_mut_slice(fdt) };

        Ok(fdt)
    }

    /// Creates an empty Flattened Device Tree with a mutable slice.
    pub fn create_empty_tree(fdt: &mut [u8]) -> Result<&mut Self> {
        libfdt::create_empty_tree(fdt)?;

        Self::from_mut_slice(fdt)
    }

    /// Wraps a slice containing a Flattened Device Tree.
    ///
    /// # Safety
    ///
    /// It is undefined to call this function on a slice that does not contain a valid device tree.
    pub const unsafe fn unchecked_from_slice(fdt: &[u8]) -> &Self {
        let self_ptr = fdt as *const _ as *const _;
        // SAFETY: The pointer is non-null, dereferenceable, and points to allocated memory.
        unsafe { &*self_ptr }
    }

    /// Wraps a mutable slice containing a Flattened Device Tree.
    ///
    /// # Safety
    ///
    /// It is undefined to call this function on a slice that does not contain a valid device tree.
    pub unsafe fn unchecked_from_mut_slice(fdt: &mut [u8]) -> &mut Self {
        let self_mut_ptr = fdt as *mut _ as *mut _;
        // SAFETY: The pointer is non-null, dereferenceable, and points to allocated memory.
        unsafe { &mut *self_mut_ptr }
    }

    /// Updates this FDT from another FDT.
    pub fn clone_from(&mut self, other: &Self) -> Result<()> {
        let new_len = other.buffer.len();
        if self.buffer.len() < new_len {
            return Err(FdtError::NoSpace);
        }

        let zeroed_len = self.totalsize().checked_sub(new_len);
        let (cloned, zeroed) = self.buffer.split_at_mut(new_len);

        cloned.clone_from_slice(&other.buffer);
        if let Some(len) = zeroed_len {
            zeroed[..len].fill(0);
        }

        Ok(())
    }

    /// Unpacks the DT to cover the whole slice it is contained in.
    pub fn unpack(&mut self) -> Result<()> {
        self.open_into_self()
    }

    /// Packs the DT to take a minimum amount of memory.
    ///
    /// Doesn't shrink the underlying memory slice.
    pub fn pack(&mut self) -> Result<()> {
        LibfdtMut::pack(self)
    }

    /// Applies a DT overlay on the base DT.
    ///
    /// # Safety
    ///
    /// As libfdt corrupts the input DT on failure, `self` should be discarded on error:
    ///
    ///     let fdt = fdt.apply_overlay(overlay)?;
    ///
    /// Furthermore, `overlay` is _always_ corrupted by libfdt and will never refer to a valid
    /// `Fdt` after this function returns and must therefore be discarded by the caller.
    pub unsafe fn apply_overlay<'a>(&'a mut self, overlay: &mut Fdt) -> Result<&'a mut Self> {
        // SAFETY: Our caller will properly discard overlay and/or self as needed.
        unsafe { self.overlay_apply(overlay) }?;

        Ok(self)
    }

    /// Returns an iterator of memory banks specified the "/memory" node.
    /// Throws an error when the "/memory" is not found in the device tree.
    ///
    /// NOTE: This does not support individual "/memory@XXXX" banks.
    pub fn memory(&self) -> Result<MemRegIterator> {
        let node = self.root().subnode(cstr!("memory"))?.ok_or(FdtError::NotFound)?;
        if node.device_type()? != Some(cstr!("memory")) {
            return Err(FdtError::BadValue);
        }
        node.reg()?.ok_or(FdtError::BadValue).map(MemRegIterator::new)
    }

    /// Returns the first memory range in the `/memory` node.
    pub fn first_memory_range(&self) -> Result<Range<usize>> {
        self.memory()?.next().ok_or(FdtError::NotFound)
    }

    /// Returns the standard /chosen node.
    pub fn chosen(&self) -> Result<Option<FdtNode>> {
        self.root().subnode(cstr!("chosen"))
    }

    /// Returns the standard /chosen node as mutable.
    pub fn chosen_mut(&mut self) -> Result<Option<FdtNodeMut>> {
        self.node_mut(cstr!("/chosen"))
    }

    /// Returns the root node of the tree.
    pub fn root(&self) -> FdtNode {
        FdtNode { fdt: self, offset: NodeOffset::ROOT }
    }

    /// Returns the standard /__symbols__ node.
    pub fn symbols(&self) -> Result<Option<FdtNode>> {
        self.root().subnode(cstr!("__symbols__"))
    }

    /// Returns the standard /__symbols__ node as mutable
    pub fn symbols_mut(&mut self) -> Result<Option<FdtNodeMut>> {
        self.node_mut(cstr!("/__symbols__"))
    }

    /// Returns a tree node by its full path.
    pub fn node(&self, path: &CStr) -> Result<Option<FdtNode>> {
        let offset = self.path_offset_namelen(path.to_bytes())?;

        Ok(offset.map(|offset| FdtNode { fdt: self, offset }))
    }

    /// Iterate over nodes with a given compatible string.
    pub fn compatible_nodes<'a>(&'a self, compatible: &'a CStr) -> Result<CompatibleIterator<'a>> {
        CompatibleIterator::new(self, compatible)
    }

    /// Returns max phandle in the tree.
    pub fn max_phandle(&self) -> Result<Phandle> {
        self.find_max_phandle()
    }

    /// Returns a node with the phandle
    pub fn node_with_phandle(&self, phandle: Phandle) -> Result<Option<FdtNode>> {
        let offset = self.node_offset_by_phandle(phandle)?;

        Ok(offset.map(|offset| FdtNode { fdt: self, offset }))
    }

    /// Returns a mutable node with the phandle
    pub fn node_mut_with_phandle(&mut self, phandle: Phandle) -> Result<Option<FdtNodeMut>> {
        let offset = self.node_offset_by_phandle(phandle)?;

        Ok(offset.map(|offset| FdtNodeMut { fdt: self, offset }))
    }

    /// Returns the mutable root node of the tree.
    pub fn root_mut(&mut self) -> FdtNodeMut {
        FdtNodeMut { fdt: self, offset: NodeOffset::ROOT }
    }

    /// Returns a mutable tree node by its full path.
    pub fn node_mut(&mut self, path: &CStr) -> Result<Option<FdtNodeMut>> {
        let offset = self.path_offset_namelen(path.to_bytes())?;

        Ok(offset.map(|offset| FdtNodeMut { fdt: self, offset }))
    }

    fn next_node_skip_subnodes(
        &self,
        node: NodeOffset,
        depth: usize,
    ) -> Result<Option<(NodeOffset, usize)>> {
        let mut iter = self.next_node(node, depth)?;
        while let Some((offset, next_depth)) = iter {
            if next_depth <= depth {
                return Ok(Some((offset, next_depth)));
            }
            iter = self.next_node(offset, next_depth)?;
        }

        Ok(None)
    }

    /// Returns the device tree as a slice (may be smaller than the containing buffer).
    pub fn as_slice(&self) -> &[u8] {
        self.as_fdt_slice()
    }

    fn get_from_ptr(&self, ptr: *const c_void, len: usize) -> Result<&[u8]> {
        get_slice_at_ptr(self.as_fdt_slice(), ptr.cast(), len).ok_or(FdtError::Internal)
    }

    /// Returns a shared pointer to the device tree.
    pub fn as_ptr(&self) -> *const c_void {
        self.buffer.as_ptr().cast()
    }

    fn header(&self) -> &FdtHeader {
        let p = self.as_ptr().cast::<libfdt_bindgen::fdt_header>();
        // SAFETY: A valid FDT (verified by constructor) must contain a valid fdt_header.
        let header = unsafe { &*p };
        header.as_ref()
    }

    fn totalsize(&self) -> usize {
        self.header().totalsize.get().try_into().unwrap()
    }
}
