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

use vmbase::fdt::pci::PciError;

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failed memory operation.
    #[error("Failed memory operation: {0}")]
    MemoryOperationFailed(#[from] vmbase::memory::MemoryTrackerError),
    /// Invalid FDT.
    #[error("Invalid FDT: {0}")]
    InvalidFdt(#[from] libfdt::FdtError),
    /// Invalid PCI.
    #[error("Invalid PCI: {0}")]
    InvalidPci(#[from] PciError),
    /// Failed VirtIO driver operation.
    #[error("Failed VirtIO driver operation: {0}")]
    VirtIODriverOperationFailed(#[from] virtio_drivers::Error),
    /// Missing socket device.
    #[error("Missing VirtIO Socket device.")]
    MissingVirtIOSocketDevice,
    /// Failed to create VirtIO Socket device.
    #[error("Failed to create VirtIO Socket device: {0}")]
    VirtIOSocketCreationFailed(virtio_drivers::Error),
    /// Failed to initialize PCI.
    #[error("Failed to initialize PCI: {0}")]
    PciInitializationFailed(PciError),
    /// Failed to serialize.
    #[error("Failed to serialize: {0}")]
    SerializationFailed(#[from] ciborium::ser::Error<virtio_drivers::Error>),
    /// Failed to deserialize.
    #[error("Failed to deserialize: {0}")]
    DeserializationFailed(#[from] ciborium::de::Error<virtio_drivers::Error>),
}
