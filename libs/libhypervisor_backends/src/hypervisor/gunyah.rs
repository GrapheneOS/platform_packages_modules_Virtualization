use super::common::Hypervisor;
use super::{DeviceAssigningHypervisor, MmioGuardedHypervisor};
use crate::mem::page_4kb_of;
use crate::{Error, Result};
use alloc::boxed::Box;
use core::mem::{size_of, MaybeUninit};
use once_cell::race::OnceBox;
use smccc::{error::success_or_error_64, hvc64};
use thiserror::Error;
use uuid::{uuid, Uuid};

const SIZE_4KB: usize = 4 << 10;
const GUNYAH_HYPERCALL_ADDRSPACE_INFO_AREA_GET_ENTRY: u32 = 0xc6008077;
const GUNYAH_HYPERCALL_ADDRSPACE_VMMIO_CONFIGURE: u32 = 0xc6008060;
const GUNYAH_VMMIO_CONFIGURE_OP_ADD_RANGE: u64 = 0;
const GUNYAH_VMMIO_CONFIGURE_OP_REMOVE_RANGE: u64 = 1;

pub(super) struct GunyahHypervisor;

// For the capid needed to make the MMIO_GUARD calls
static ROOTVM_ADDRSPACE_CAP: OnceBox<Option<u64>> = OnceBox::new();
// 15:0      id
// 31:16     owner: 0x2 = rootvm
const ADDRSPACE_INFO_AREA_ROOTVM_ADDRSPACE_ENTRY: u32 = 0x2 << 16;
#[repr(packed)]
struct RootvmAddrspaceCap {
    addrspace_cap: u64,
    _addrspace_rights: u32,
    _res0: u32,
}

/// Error from a Gunyah HVC call.
#[derive(Copy, Clone, Debug, Eq, Error, PartialEq)]
pub enum GunyahError {
    /// The call is not supported by the implementation.
    #[error("Gunyah call not supported")]
    NotSupported,
    /// Invalid args passed to Gunyah
    #[error("Argument to Gunyah call is invalid")]
    ArgumentInvalid,
    /// No rescources to complete the request
    #[error("No Resources to complete Gunyah call")]
    NoResources,
    /// Address provided wraps around
    #[error("Address wraps around")]
    AddressOverflow,
    /// Address provided is incorrect
    #[error("Address to map might be already mapped")]
    Busy,
    /// Address provided is incorrect
    #[error("Address to remove map might not be mapped")]
    Idle,
    /// There was an unexpected return value.
    #[error("Unknown return value from Gunyah {0} ({0:#x})")]
    Unknown(i64),
}

impl From<i64> for GunyahError {
    fn from(value: i64) -> Self {
        match value {
            -1 => GunyahError::NotSupported,
            1 => GunyahError::ArgumentInvalid,
            10 => GunyahError::NoResources,
            20 => GunyahError::AddressOverflow,
            31 => GunyahError::Busy,
            32 => GunyahError::Idle,
            _ => GunyahError::Unknown(value),
        }
    }
}

impl From<i32> for GunyahError {
    fn from(value: i32) -> Self {
        i64::from(value).into()
    }
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

    fn as_mmio_guard(&self) -> Option<&dyn MmioGuardedHypervisor> {
        if let Some(_addr_capid) = get_addrspc_cap_id() {
            Some(self)
        } else {
            None
        }
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

impl MmioGuardedHypervisor for GunyahHypervisor {
    fn enroll(&self) -> Result<()> {
        Ok(())
    }

    fn map(&self, addr: usize) -> Result<()> {
        let addr_capid = get_addrspc_cap_id().expect("MMIO GUARD is not supported");
        let mut args = [0u64; 17];
        args[0] = addr_capid;
        args[1] = page_4kb_of(addr).try_into().unwrap();
        args[2] = SIZE_4KB as u64;
        args[3] = GUNYAH_VMMIO_CONFIGURE_OP_ADD_RANGE;
        args[4] = 0; /* reserved */

        let ret = hvc64(GUNYAH_HYPERCALL_ADDRSPACE_VMMIO_CONFIGURE, args)[0];
        match success_or_error_64(ret) {
            /*
             * This could happen when we have to support the hard-coded VMMIO ranges
             * until all the VMs are updated. This means that Gunyah is already
             * allowing the traps in this range.
             */
            Err(GunyahError::Busy) | Ok(_) => Ok(()),
            Err(_e) => Err(Error::GunyahError(GunyahError::from(ret as i64))),
        }
    }

    fn unmap(&self, addr: usize) -> Result<()> {
        let addr_capid = get_addrspc_cap_id().expect("MMIO GUARD is not supported");
        let mut args = [0u64; 17];
        args[0] = addr_capid;
        args[1] = page_4kb_of(addr).try_into().unwrap();
        args[2] = SIZE_4KB as u64;
        args[3] = GUNYAH_VMMIO_CONFIGURE_OP_REMOVE_RANGE;
        args[4] = 0; /* reserved */

        let ret = hvc64(GUNYAH_HYPERCALL_ADDRSPACE_VMMIO_CONFIGURE, args)[0];
        match success_or_error_64(ret) {
            /*
             * ARGUMENT_INVALID:
             * If corresponding MAP call had failed, then this won't succeed either.
             * This could happen when we have to support the hard-coded VMMIO ranges
             * until all the VMs are updated.
             * On the contrary, if the MAP call succeeded, then this should also succeed.
             * IDLE:
             * Didn't have anything to remove. Its harmless.
             */
            Err(GunyahError::ArgumentInvalid) | Err(GunyahError::Idle) | Ok(_) => Ok(()),

            Err(_e) => Err(Error::GunyahError(GunyahError::from(ret as i64))),
        }
    }

    fn granule(&self) -> Result<usize> {
        Ok(SIZE_4KB)
    }
}

fn gunyah_hypercall_addrspace_get_info_area_entry() -> Option<u64> {
    let mut entry = MaybeUninit::<RootvmAddrspaceCap>::zeroed();
    let entry_size: u64 = size_of::<RootvmAddrspaceCap>().try_into().unwrap();
    let mut args = [0u64; 17];
    args[0] = ADDRSPACE_INFO_AREA_ROOTVM_ADDRSPACE_ENTRY as u64;
    args[1] = entry.as_mut_ptr() as u64;
    args[2] = entry_size;
    args[3] = 0; /* reserved */

    let ret = hvc64(GUNYAH_HYPERCALL_ADDRSPACE_INFO_AREA_GET_ENTRY, args);
    if ret[0] != 0 || ret[1] != entry_size {
        None
    } else {
        // Gunyah will fill up the structure with the data
        // SAFETY: Gunyah will return the size filled and we validate it before accessing.
        Some(unsafe { entry.assume_init().addrspace_cap })
    }
}

fn get_addrspc_cap_id() -> Option<u64> {
    *ROOTVM_ADDRSPACE_CAP.get_or_init(|| Box::new(gunyah_hypercall_addrspace_get_info_area_entry()))
}
