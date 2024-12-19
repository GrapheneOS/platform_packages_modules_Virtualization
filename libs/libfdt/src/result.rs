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

//! Rust types related to the libfdt C integer results.

use core::error;
use core::ffi::{c_int, c_uint};
use core::fmt;
use core::result;

/// Error type corresponding to libfdt error codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FdtError {
    /// FDT_ERR_NOTFOUND
    NotFound,
    /// FDT_ERR_EXISTS
    Exists,
    /// FDT_ERR_NOSPACE
    NoSpace,
    /// FDT_ERR_BADOFFSET
    BadOffset,
    /// FDT_ERR_BADPATH
    BadPath,
    /// FDT_ERR_BADPHANDLE
    BadPhandle,
    /// FDT_ERR_BADSTATE
    BadState,
    /// FDT_ERR_TRUNCATED
    Truncated,
    /// FDT_ERR_BADMAGIC
    BadMagic,
    /// FDT_ERR_BADVERSION
    BadVersion,
    /// FDT_ERR_BADSTRUCTURE
    BadStructure,
    /// FDT_ERR_BADLAYOUT
    BadLayout,
    /// FDT_ERR_INTERNAL
    Internal,
    /// FDT_ERR_BADNCELLS
    BadNCells,
    /// FDT_ERR_BADVALUE
    BadValue,
    /// FDT_ERR_BADOVERLAY
    BadOverlay,
    /// FDT_ERR_NOPHANDLES
    NoPhandles,
    /// FDT_ERR_BADFLAGS
    BadFlags,
    /// FDT_ERR_ALIGNMENT
    Alignment,
    /// Unexpected error code
    Unknown(i32),
}

impl fmt::Display for FdtError {
    /// Prints error messages from libfdt.h documentation.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "The requested node or property does not exist"),
            Self::Exists => write!(f, "Attempted to create an existing node or property"),
            Self::NoSpace => write!(f, "Insufficient buffer space to contain the expanded tree"),
            Self::BadOffset => write!(f, "Structure block offset is out-of-bounds or invalid"),
            Self::BadPath => write!(f, "Badly formatted path"),
            Self::BadPhandle => write!(f, "Invalid phandle length or value"),
            Self::BadState => write!(f, "Received incomplete device tree"),
            Self::Truncated => write!(f, "Device tree or sub-block is improperly terminated"),
            Self::BadMagic => write!(f, "Device tree header missing its magic number"),
            Self::BadVersion => write!(f, "Device tree has a version which can't be handled"),
            Self::BadStructure => write!(f, "Device tree has a corrupt structure block"),
            Self::BadLayout => write!(f, "Device tree sub-blocks in unsupported order"),
            Self::Internal => write!(f, "libfdt has failed an internal assertion"),
            Self::BadNCells => write!(f, "Bad format or value of #address-cells or #size-cells"),
            Self::BadValue => write!(f, "Unexpected property value"),
            Self::BadOverlay => write!(f, "Overlay cannot be applied"),
            Self::NoPhandles => write!(f, "Device tree doesn't have any phandle available anymore"),
            Self::BadFlags => write!(f, "Invalid flag or invalid combination of flags"),
            Self::Alignment => write!(f, "Device tree base address is not 8-byte aligned"),
            Self::Unknown(e) => write!(f, "Unknown libfdt error '{e}'"),
        }
    }
}

impl error::Error for FdtError {}

/// Result type with FdtError enum.
pub type Result<T> = result::Result<T, FdtError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FdtRawResult(c_int);

impl From<c_int> for FdtRawResult {
    fn from(value: c_int) -> Self {
        Self(value)
    }
}

impl TryFrom<FdtRawResult> for c_int {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        use libfdt_bindgen::{
            FDT_ERR_ALIGNMENT, FDT_ERR_BADFLAGS, FDT_ERR_BADLAYOUT, FDT_ERR_BADMAGIC,
            FDT_ERR_BADNCELLS, FDT_ERR_BADOFFSET, FDT_ERR_BADOVERLAY, FDT_ERR_BADPATH,
            FDT_ERR_BADPHANDLE, FDT_ERR_BADSTATE, FDT_ERR_BADSTRUCTURE, FDT_ERR_BADVALUE,
            FDT_ERR_BADVERSION, FDT_ERR_EXISTS, FDT_ERR_INTERNAL, FDT_ERR_NOPHANDLES,
            FDT_ERR_NOSPACE, FDT_ERR_NOTFOUND, FDT_ERR_TRUNCATED,
        };
        match res.0 {
            x if x >= 0 => Ok(x),
            x if x == -(FDT_ERR_NOTFOUND as c_int) => Err(FdtError::NotFound),
            x if x == -(FDT_ERR_EXISTS as c_int) => Err(FdtError::Exists),
            x if x == -(FDT_ERR_NOSPACE as c_int) => Err(FdtError::NoSpace),
            x if x == -(FDT_ERR_BADOFFSET as c_int) => Err(FdtError::BadOffset),
            x if x == -(FDT_ERR_BADPATH as c_int) => Err(FdtError::BadPath),
            x if x == -(FDT_ERR_BADPHANDLE as c_int) => Err(FdtError::BadPhandle),
            x if x == -(FDT_ERR_BADSTATE as c_int) => Err(FdtError::BadState),
            x if x == -(FDT_ERR_TRUNCATED as c_int) => Err(FdtError::Truncated),
            x if x == -(FDT_ERR_BADMAGIC as c_int) => Err(FdtError::BadMagic),
            x if x == -(FDT_ERR_BADVERSION as c_int) => Err(FdtError::BadVersion),
            x if x == -(FDT_ERR_BADSTRUCTURE as c_int) => Err(FdtError::BadStructure),
            x if x == -(FDT_ERR_BADLAYOUT as c_int) => Err(FdtError::BadLayout),
            x if x == -(FDT_ERR_INTERNAL as c_int) => Err(FdtError::Internal),
            x if x == -(FDT_ERR_BADNCELLS as c_int) => Err(FdtError::BadNCells),
            x if x == -(FDT_ERR_BADVALUE as c_int) => Err(FdtError::BadValue),
            x if x == -(FDT_ERR_BADOVERLAY as c_int) => Err(FdtError::BadOverlay),
            x if x == -(FDT_ERR_NOPHANDLES as c_int) => Err(FdtError::NoPhandles),
            x if x == -(FDT_ERR_BADFLAGS as c_int) => Err(FdtError::BadFlags),
            x if x == -(FDT_ERR_ALIGNMENT as c_int) => Err(FdtError::Alignment),
            x => Err(FdtError::Unknown(x)),
        }
    }
}

impl TryFrom<FdtRawResult> for Option<c_int> {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        match res.try_into() {
            Ok(n) => Ok(Some(n)),
            Err(FdtError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl TryFrom<FdtRawResult> for c_uint {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        Ok(c_int::try_from(res)?.try_into().unwrap())
    }
}

impl TryFrom<FdtRawResult> for usize {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        Ok(c_int::try_from(res)?.try_into().unwrap())
    }
}

impl TryFrom<FdtRawResult> for Option<usize> {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        match res.try_into() {
            Ok(n) => Ok(Some(n)),
            Err(FdtError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl TryFrom<FdtRawResult> for () {
    type Error = FdtError;

    fn try_from(res: FdtRawResult) -> Result<Self> {
        match res.try_into()? {
            0 => Ok(()),
            n => Err(FdtError::Unknown(n)),
        }
    }
}
