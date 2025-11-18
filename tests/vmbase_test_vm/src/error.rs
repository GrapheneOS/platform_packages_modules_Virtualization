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
use vmbase::fdt::pci::PciError;
use vmbase::memory::MemoryTrackerError;

pub type Result<T> = core::result::Result<T, Error>;

type VirtioDriverError = virtio_drivers::Error;
type CiboriumSerError = ciborium::ser::Error<virtio_drivers::Error>;
type CiboriumDeError = ciborium::de::Error<virtio_drivers::Error>;

#[derive(Debug)]
pub enum Error {
    /// Failed memory operation.
    MemoryOperationFailed(MemoryTrackerError),
    /// Invalid FDT.
    InvalidFdt(FdtError),
    /// Invalid PCI.
    InvalidPci(PciError),
    /// Failed VirtIO driver operation.
    VirtIODriverOperationFailed(VirtioDriverError),
    /// Missing socket device.
    MissingVirtIOSocketDevice,
    /// Failed to create VirtIO Socket device.
    VirtIOSocketCreationFailed(VirtioDriverError),
    /// Failed to initialize PCI.
    PciInitializationFailed(PciError),
    /// Failed to serialize.
    SerializationFailed(CiboriumSerError),
    /// Failed to deserialize.
    DeserializationFailed(CiboriumDeError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::MemoryOperationFailed(e) => write!(f, "Failed memory operation: {e}"),
            Self::InvalidFdt(e) => write!(f, "Invalid FDT: {e}"),
            Self::PciInitializationFailed(e) => write!(f, "Failed to initialize PCI: {e}"),
            Self::VirtIOSocketCreationFailed(e) => {
                write!(f, "Failed to create VirtIO Socket device: {e}")
            }
            Self::MissingVirtIOSocketDevice => write!(f, "Missing VirtIO Socket device."),
            Self::VirtIODriverOperationFailed(e) => {
                write!(f, "Failed VirtIO driver operation: {e}")
            }
            Self::SerializationFailed(e) => write!(f, "Failed to serialize: {e}"),
            Self::DeserializationFailed(e) => write!(f, "Failed to deserialize: {e}"),
            Self::InvalidPci(e) => write!(f, "Invalid PCI: {e}"),
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

impl From<PciError> for Error {
    fn from(e: PciError) -> Self {
        Self::InvalidPci(e)
    }
}

impl From<virtio_drivers::Error> for Error {
    fn from(e: VirtioDriverError) -> Self {
        Self::VirtIODriverOperationFailed(e)
    }
}

impl From<CiboriumSerError> for Error {
    fn from(e: CiboriumSerError) -> Self {
        Self::SerializationFailed(e)
    }
}

impl From<CiboriumDeError> for Error {
    fn from(e: CiboriumDeError) -> Self {
        Self::DeserializationFailed(e)
    }
}
