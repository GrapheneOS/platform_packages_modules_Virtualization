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

//! Iterators over cells, and various layers on top of them.

use crate::Fdt;
use crate::FdtError;
use crate::FdtNode;
use crate::FdtProperty;
use crate::{AddrCells, SizeCells};
use core::ffi::CStr;
use core::marker::PhantomData;
use core::{mem::size_of, ops::Range, slice::ChunksExact};

use zerocopy::transmute;

/// Iterator over nodes sharing a same compatible string.
pub struct CompatibleIterator<'a> {
    node: FdtNode<'a>,
    compatible: &'a CStr,
}

impl<'a> CompatibleIterator<'a> {
    pub(crate) fn new(fdt: &'a Fdt, compatible: &'a CStr) -> Result<Self, FdtError> {
        let node = fdt.root();
        Ok(Self { node, compatible })
    }
}

impl<'a> Iterator for CompatibleIterator<'a> {
    type Item = FdtNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.node.next_compatible(self.compatible).ok()?;

        if let Some(node) = next {
            self.node = node;
        }

        next
    }
}

/// Iterator over cells of a DT property.
#[derive(Debug)]
pub struct CellIterator<'a> {
    chunks: ChunksExact<'a, u8>,
}

impl<'a> CellIterator<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        const CHUNK_SIZE: usize = size_of::<<CellIterator as Iterator>::Item>();

        Self { chunks: bytes.chunks_exact(CHUNK_SIZE) }
    }
}

impl Iterator for CellIterator<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Self::Item::from_be_bytes(self.chunks.next()?.try_into().ok()?))
    }
}

/// Iterator over a 'reg' property of a DT node.
#[derive(Debug)]
pub struct RegIterator<'a> {
    cells: CellIterator<'a>,
    addr_cells: AddrCells,
    size_cells: SizeCells,
}

/// Represents a contiguous region within the address space defined by the parent bus.
/// Commonly means the offsets and lengths of MMIO blocks, but may have a different meaning on some
/// bus types. Addresses in the address space defined by the root node are CPU real addresses.
#[derive(Copy, Clone, Debug)]
pub struct Reg<T> {
    /// Base address of the region.
    pub addr: T,
    /// Size of the region (optional).
    pub size: Option<T>,
}

impl<T: TryInto<usize>> TryFrom<Reg<T>> for Range<usize> {
    type Error = FdtError;

    fn try_from(reg: Reg<T>) -> Result<Self, Self::Error> {
        let addr = to_usize(reg.addr)?;
        let size = to_usize(reg.size.ok_or(FdtError::NotFound)?)?;
        let end = addr.checked_add(size).ok_or(FdtError::BadValue)?;
        Ok(addr..end)
    }
}

fn to_usize<T: TryInto<usize>>(num: T) -> Result<usize, FdtError> {
    num.try_into().map_err(|_| FdtError::BadValue)
}

impl<'a> RegIterator<'a> {
    pub(crate) fn new(
        cells: CellIterator<'a>,
        addr_cells: AddrCells,
        size_cells: SizeCells,
    ) -> Self {
        Self { cells, addr_cells, size_cells }
    }
}

impl Iterator for RegIterator<'_> {
    type Item = Reg<u64>;

    fn next(&mut self) -> Option<Self::Item> {
        let addr = FromAddrCells::from_addr_cells(&mut self.cells, self.addr_cells)?;
        // If the parent node specifies a value of 0 for #size-cells, 'size' shall be omitted.
        let size = if self.size_cells == SizeCells::None {
            None
        } else {
            Some(FromSizeCells::from_size_cells(&mut self.cells, self.size_cells)?)
        };

        Some(Self::Item { addr, size })
    }
}

impl Reg<u64> {
    const NUM_CELLS: usize = 2;
    /// Converts addr and (optional) size to the format that is consumable by libfdt.
    pub fn to_cells(
        &self,
    ) -> ([u8; Self::NUM_CELLS * size_of::<u32>()], Option<[u8; Self::NUM_CELLS * size_of::<u32>()]>)
    {
        let addr = transmute!([((self.addr >> 32) as u32).to_be(), (self.addr as u32).to_be()]);
        let size =
            self.size.map(|sz| transmute!([((sz >> 32) as u32).to_be(), (sz as u32).to_be()]));

        (addr, size)
    }
}

/// Iterator over the address ranges defined by the /memory/ node.
#[derive(Debug)]
pub struct MemRegIterator<'a> {
    reg: RegIterator<'a>,
}

impl<'a> MemRegIterator<'a> {
    pub(crate) fn new(reg: RegIterator<'a>) -> Self {
        Self { reg }
    }
}

impl Iterator for MemRegIterator<'_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.reg.next()?;
        let addr = usize::try_from(next.addr).ok()?;
        let size = usize::try_from(next.size?).ok()?;

        Some(addr..addr.checked_add(size)?)
    }
}

/// Iterator over the 'ranges' property of a DT node.
#[derive(Debug)]
pub struct RangesIterator<'a, A, P, S> {
    cells: CellIterator<'a>,
    addr_cells: AddrCells,
    parent_addr_cells: AddrCells,
    size_cells: SizeCells,
    _addr: PhantomData<A>,
    _parent_addr: PhantomData<P>,
    _size: PhantomData<S>,
}

/// An address range from the 'ranges' property of a DT node.
#[derive(Clone, Debug, Default)]
pub struct AddressRange<A, P, S> {
    /// The physical address of the range within the child bus's address space.
    pub addr: A,
    /// The physical address of the range in the parent bus's address space.
    pub parent_addr: P,
    /// The size of the range in the child's address space.
    pub size: S,
}

impl<'a, A, P, S> RangesIterator<'a, A, P, S> {
    pub(crate) fn new(
        cells: CellIterator<'a>,
        addr_cells: AddrCells,
        parent_addr_cells: AddrCells,
        size_cells: SizeCells,
    ) -> Self {
        Self {
            cells,
            addr_cells,
            parent_addr_cells,
            size_cells,
            _addr: Default::default(),
            _parent_addr: Default::default(),
            _size: Default::default(),
        }
    }
}

impl<A: FromAddrCells, P: FromAddrCells, S: FromSizeCells> Iterator
    for RangesIterator<'_, A, P, S>
{
    type Item = AddressRange<A, P, S>;

    fn next(&mut self) -> Option<Self::Item> {
        let addr = FromAddrCells::from_addr_cells(&mut self.cells, self.addr_cells)?;
        let parent_addr = FromAddrCells::from_addr_cells(&mut self.cells, self.parent_addr_cells)?;
        let size = FromSizeCells::from_size_cells(&mut self.cells, self.size_cells)?;
        Some(AddressRange { addr, parent_addr, size })
    }
}

trait FromAddrCells: Sized {
    fn from_addr_cells(cells: &mut CellIterator, cell_count: AddrCells) -> Option<Self>;
}

impl FromAddrCells for u64 {
    fn from_addr_cells(cells: &mut CellIterator, cell_count: AddrCells) -> Option<Self> {
        Some(match cell_count {
            AddrCells::Single => cells.next()?.into(),
            AddrCells::Double => (cells.next()? as Self) << 32 | cells.next()? as Self,
            _ => panic!("Invalid addr_cells {:?} for u64", cell_count),
        })
    }
}

impl FromAddrCells for (u32, u64) {
    fn from_addr_cells(cells: &mut CellIterator, cell_count: AddrCells) -> Option<Self> {
        Some(match cell_count {
            AddrCells::Triple => {
                (cells.next()?, (cells.next()? as u64) << 32 | cells.next()? as u64)
            }
            _ => panic!("Invalid addr_cells {:?} for (u32, u64)", cell_count),
        })
    }
}

trait FromSizeCells: Sized {
    fn from_size_cells(cells: &mut CellIterator, cell_count: SizeCells) -> Option<Self>;
}

impl FromSizeCells for u64 {
    fn from_size_cells(cells: &mut CellIterator, cell_count: SizeCells) -> Option<Self> {
        Some(match cell_count {
            SizeCells::Single => cells.next()?.into(),
            SizeCells::Double => (cells.next()? as Self) << 32 | cells.next()? as Self,
            _ => panic!("Invalid size_cells {:?} for u64", cell_count),
        })
    }
}

impl AddressRange<(u32, u64), u64, u64> {
    const SIZE_CELLS: usize = 7;
    /// Converts to the format that is consumable by libfdt
    pub fn to_cells(&self) -> [u8; Self::SIZE_CELLS * size_of::<u32>()] {
        let buf = [
            self.addr.0.to_be(),
            ((self.addr.1 >> 32) as u32).to_be(),
            (self.addr.1 as u32).to_be(),
            ((self.parent_addr >> 32) as u32).to_be(),
            (self.parent_addr as u32).to_be(),
            ((self.size >> 32) as u32).to_be(),
            (self.size as u32).to_be(),
        ];

        transmute!(buf)
    }
}

/// Iterator over subnodes
#[derive(Debug)]
pub struct SubnodeIterator<'a> {
    subnode: Option<FdtNode<'a>>,
}

impl<'a> SubnodeIterator<'a> {
    pub(crate) fn new(node: &FdtNode<'a>) -> Result<Self, FdtError> {
        let subnode = node.first_subnode()?;

        Ok(Self { subnode })
    }
}

impl<'a> Iterator for SubnodeIterator<'a> {
    type Item = FdtNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let res = self.subnode;

        self.subnode = self.subnode.and_then(|node| node.next_subnode().ok()?);

        res
    }
}

/// Iterator over descendants
#[derive(Debug)]
pub struct DescendantsIterator<'a> {
    node: Option<(FdtNode<'a>, usize)>,
}

impl<'a> DescendantsIterator<'a> {
    pub(crate) fn new(node: &FdtNode<'a>) -> Self {
        Self { node: Some((*node, 0)) }
    }
}

impl<'a> Iterator for DescendantsIterator<'a> {
    type Item = (FdtNode<'a>, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let (node, depth) = self.node?;
        self.node = node.next_node(depth).ok().flatten().filter(|(_, depth)| *depth > 0);

        self.node
    }
}

/// Iterator over properties
#[derive(Debug)]
pub struct PropertyIterator<'a> {
    prop: Option<FdtProperty<'a>>,
}

impl<'a> PropertyIterator<'a> {
    pub(crate) fn new(node: &'a FdtNode) -> Result<Self, FdtError> {
        let prop = node.first_property()?;

        Ok(Self { prop })
    }
}

impl<'a> Iterator for PropertyIterator<'a> {
    type Item = FdtProperty<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let res = self.prop;

        self.prop = res?.next_property().ok()?;

        res
    }
}
