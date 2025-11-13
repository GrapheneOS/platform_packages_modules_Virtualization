// Copyright 2025, The Android Open Source Project
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

//! Errors thrown by the test VM.

// TODO(ioffe): consolidate with guest/service_vm/src/error.rs and put as a library?

use core::fmt;
use libfdt::FdtError;
use vmbase::memory::MemoryTrackerError;

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// Failed memory operation.
    MemoryOperationFailed(MemoryTrackerError),
    /// Invalid FDT.
    InvalidFdt(FdtError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::MemoryOperationFailed(e) => write!(f, "Failed memory operation: {e}"),
            Self::InvalidFdt(e) => write!(f, "Invalid FDT: {e}"),
        }
    }
}

impl From<MemoryTrackerError> for Error {
    fn from(e: MemoryTrackerError) -> Self {
        Self::MemoryOperationFailed(e)
    }
}

impl From<FdtError> for Error {
    fn from(e: FdtError) -> Self {
        Self::InvalidFdt(e)
    }
}
