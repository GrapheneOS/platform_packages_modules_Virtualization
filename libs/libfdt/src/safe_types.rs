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

//! Safe zero-cost wrappers around integer values used by libfdt.

use core::ffi::c_int;
use core::mem::offset_of;

use crate::result::FdtRawResult;
use crate::{FdtError, Result};

use zerocopy::byteorder::big_endian;
use zerocopy::{FromBytes, FromZeroes};

macro_rules! assert_offset_eq {
    // TODO(const_feature(assert_eq)): assert_eq!()
    ($t:ty, $u:ty, $id:ident) => {
        static_assertions::const_assert_eq!(offset_of!($t, $id), offset_of!($u, $id));
    };
}

/// Thin wrapper around `libfdt_bindgen::fdt_header` for transparent endianness handling.
#[repr(C)]
#[derive(Debug, FromZeroes, FromBytes)]
pub struct FdtHeader {
    /// magic word FDT_MAGIC
    pub magic: big_endian::U32,
    /// total size of DT block
    pub totalsize: big_endian::U32,
    /// offset to structure
    pub off_dt_struct: big_endian::U32,
    /// offset to strings
    pub off_dt_strings: big_endian::U32,
    /// offset to memory reserve map
    pub off_mem_rsvmap: big_endian::U32,
    /// format version
    pub version: big_endian::U32,
    /// last compatible version
    pub last_comp_version: big_endian::U32,
    /* version 2 fields below */
    /// Which physical CPU id we're booting on
    pub boot_cpuid_phys: big_endian::U32,
    /* version 3 fields below */
    /// size of the strings block
    pub size_dt_strings: big_endian::U32,
    /* version 17 fields below */
    /// size of the structure block
    pub size_dt_struct: big_endian::U32,
}
assert_offset_eq!(libfdt_bindgen::fdt_header, FdtHeader, magic);
assert_offset_eq!(libfdt_bindgen::fdt_header, FdtHeader, totalsize);
assert_offset_eq!(libfdt_bindgen::fdt_header, FdtHeader, off_dt_struct);
assert_offset_eq!(libfdt_bindgen::fdt_header, FdtHeader, off_dt_strings);
assert_offset_eq!(libfdt_bindgen::fdt_header, FdtHeader, off_mem_rsvmap);
assert_offset_eq!(libfdt_bindgen::fdt_header, FdtHeader, version);
assert_offset_eq!(libfdt_bindgen::fdt_header, FdtHeader, last_comp_version);
assert_offset_eq!(libfdt_bindgen::fdt_header, FdtHeader, boot_cpuid_phys);
assert_offset_eq!(libfdt_bindgen::fdt_header, FdtHeader, size_dt_strings);
assert_offset_eq!(libfdt_bindgen::fdt_header, FdtHeader, size_dt_struct);

impl AsRef<FdtHeader> for libfdt_bindgen::fdt_header {
    fn as_ref(&self) -> &FdtHeader {
        let ptr = self as *const _ as *const _;
        // SAFETY: Types have the same layout (u32 and U32 have the same storage) and alignment.
        unsafe { &*ptr }
    }
}

/// Wrapper guaranteed to contain a valid phandle.
#[repr(transparent)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct Phandle(u32);

impl Phandle {
    /// Minimum valid value for device tree phandles.
    pub const MIN: Self = Self(1);
    /// Maximum valid value for device tree phandles.
    pub const MAX: Self = Self(libfdt_bindgen::FDT_MAX_PHANDLE);

    /// Creates a new Phandle
    pub const fn new(value: u32) -> Option<Self> {
        if Self::MIN.0 <= value && value <= Self::MAX.0 {
            Some(Self(value))
        } else {
            None
        }
    }
}

impl From<Phandle> for u32 {
    fn from(phandle: Phandle) -> u32 {
        phandle.0
    }
}

impl TryFrom<u32> for Phandle {
    type Error = FdtError;

    fn try_from(value: u32) -> Result<Self> {
        Self::new(value).ok_or(FdtError::BadPhandle)
    }
}

impl TryFrom<FdtRawResult> for Phandle {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        Self::new(res.try_into()?).ok_or(FdtError::BadPhandle)
    }
}

/// Safe zero-cost wrapper around libfdt device tree node offsets.
///
/// This type should only be obtained from properly wrapped successful libfdt calls.
#[repr(transparent)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct NodeOffset(c_int);

impl NodeOffset {
    /// Offset of the root node; 0, by definition.
    pub const ROOT: Self = Self(0);
}

impl TryFrom<FdtRawResult> for NodeOffset {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        Ok(Self(res.try_into()?))
    }
}

impl TryFrom<FdtRawResult> for Option<NodeOffset> {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        match res.try_into() {
            Ok(n) => Ok(Some(n)),
            Err(FdtError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl From<NodeOffset> for c_int {
    fn from(offset: NodeOffset) -> Self {
        offset.0
    }
}

/// Safe zero-cost wrapper around libfdt device tree property offsets.
///
/// This type should only be obtained from properly wrapped successful libfdt calls.
#[repr(transparent)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct PropOffset(c_int);

impl TryFrom<FdtRawResult> for PropOffset {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        Ok(Self(res.try_into()?))
    }
}

impl TryFrom<FdtRawResult> for Option<PropOffset> {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        match res.try_into() {
            Ok(n) => Ok(Some(n)),
            Err(FdtError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl From<PropOffset> for c_int {
    fn from(offset: PropOffset) -> Self {
        offset.0
    }
}

/// Safe zero-cost wrapper around libfdt device tree string offsets.
///
/// This type should only be obtained from properly wrapped successful libfdt calls.
#[repr(transparent)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct StringOffset(pub c_int); // TODO(ptosi): Move fdt_property wrapper here and remove pub.

impl TryFrom<FdtRawResult> for StringOffset {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        Ok(Self(res.try_into()?))
    }
}

impl TryFrom<FdtRawResult> for Option<StringOffset> {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        match res.try_into() {
            Ok(n) => Ok(Some(n)),
            Err(FdtError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl From<StringOffset> for c_int {
    fn from(offset: StringOffset) -> Self {
        offset.0
    }
}
