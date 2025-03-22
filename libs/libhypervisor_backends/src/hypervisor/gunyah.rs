use super::common::Hypervisor;
use super::DeviceAssigningHypervisor;
use crate::{Error, Result};
use thiserror::Error;
use uuid::{uuid, Uuid};

const SIZE_4KB: usize = 4 << 10;

pub(super) struct GunyahHypervisor;

/// Error from a Gunyah HVC call.
#[derive(Copy, Clone, Debug, Eq, Error, PartialEq)]
pub enum GunyahError {
    /// The call is not supported by the implementation.
    #[error("Gunyah call not supported")]
    NotSupported,
}

impl GunyahHypervisor {
    pub const UUID: Uuid = uuid!("c1d58fcd-a453-5fdb-9265-ce36673d5f14");
}

impl Hypervisor for GunyahHypervisor {
    fn as_device_assigner(&self) -> Option<&dyn DeviceAssigningHypervisor> {
        Some(self)
    }

    fn get_granule_size(&self) -> Option<usize> {
        Some(SIZE_4KB)
    }
}

impl DeviceAssigningHypervisor for GunyahHypervisor {
    fn get_phys_mmio_token(&self, base_ipa: u64) -> Result<u64> {
        // PA = IPA for now.
        Ok(base_ipa)
    }

    fn get_phys_iommu_token(&self, _pviommu_id: u64, _vsid: u64) -> Result<(u64, u64)> {
        Err(Error::GunyahError(GunyahError::NotSupported))
    }
}
