// Copyright 2024, The Android Open Source Project
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

//! paging stubs to satisfy requirement of memory module

// use crate::MapError;
use core::fmt::{self, Debug, Display, Formatter};
use core::ops::{Add, Range, Sub};
use zerocopy::{FromBytes, IntoBytes};

/// The number of used bits for 2^12 = 4096 bytes
pub const PAGE_SHIFT: usize = 12;

/// The page size in bytes assumed by this library, 4 KiB.
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;

/// x86_64 memory Virtual Address
#[derive(IntoBytes, FromBytes, Copy, Clone, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct VirtualAddress(pub usize);

impl Display for VirtualAddress {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:#018x}", self.0)
    }
}

impl Debug for VirtualAddress {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "VirtualAddress({self})")
    }
}

impl Sub for VirtualAddress {
    type Output = usize;

    fn sub(self, other: Self) -> Self::Output {
        self.0 - other.0
    }
}

impl Add<usize> for VirtualAddress {
    type Output = Self;

    fn add(self, other: usize) -> Self {
        Self(self.0 + other)
    }
}

impl Sub<usize> for VirtualAddress {
    type Output = Self;

    fn sub(self, other: usize) -> Self {
        Self(self.0 - other)
    }
}

/// A range of virtual addresses which may be mapped in a page table.
#[derive(Clone, Eq, PartialEq)]
pub struct MemoryRegion(Range<VirtualAddress>);

/// An x86-64 physical address.
#[derive(IntoBytes, FromBytes, Copy, Clone, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct PhysicalAddress(pub usize);

impl Display for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:#018x}", self.0)
    }
}

impl Debug for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "PhysicalAddress({self})")
    }
}

impl Sub for PhysicalAddress {
    type Output = usize;

    fn sub(self, other: Self) -> Self::Output {
        self.0 - other.0
    }
}

impl Add<usize> for PhysicalAddress {
    type Output = Self;

    fn add(self, other: usize) -> Self {
        Self(self.0 + other)
    }
}

impl Sub<usize> for PhysicalAddress {
    type Output = Self;

    fn sub(self, other: usize) -> Self {
        Self(self.0 - other)
    }
}

impl MemoryRegion {
    /// Constructs a new `MemoryRegion` for the given range of virtual addresses.
    ///
    /// The start is inclusive and the end is exclusive. Both will be aligned to the [`PAGE_SIZE`],
    /// with the start being rounded down and the end being rounded up.
    pub const fn new(start: usize, end: usize) -> MemoryRegion {
        MemoryRegion(
            VirtualAddress(align_down(start, PAGE_SIZE))..VirtualAddress(align_up(end, PAGE_SIZE)),
        )
    }

    /// Returns the first virtual address of the memory range.
    pub const fn start(&self) -> VirtualAddress {
        self.0.start
    }

    /// Returns the first virtual address after the memory range.
    pub const fn end(&self) -> VirtualAddress {
        self.0.end
    }

    /// Returns the length of the memory region in bytes.
    pub const fn len(&self) -> usize {
        self.0.end.0 - self.0.start.0
    }

    /// Returns whether the memory region contains exactly 0 bytes.
    pub const fn is_empty(&self) -> bool {
        self.0.start.0 == self.0.end.0
    }
}

impl From<Range<VirtualAddress>> for MemoryRegion {
    fn from(range: Range<VirtualAddress>) -> Self {
        Self::new(range.start.0, range.end.0)
    }
}

impl Display for MemoryRegion {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}..{}", self.0.start, self.0.end)
    }
}

impl Debug for MemoryRegion {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        Display::fmt(self, f)
    }
}

const fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}

const fn align_up(value: usize, alignment: usize) -> usize {
    ((value - 1) | (alignment - 1)) + 1
}
