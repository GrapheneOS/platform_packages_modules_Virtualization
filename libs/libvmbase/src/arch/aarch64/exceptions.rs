// Copyright 2023, The Android Open Source Project
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

//! Helper functions and structs for exception handlers.

use crate::{
    arch::{
        aarch64::layout::UART_PAGE_ADDR,
        platform::{emergency_uart, DEFAULT_EMERGENCY_CONSOLE_INDEX},
        VirtualAddress,
    },
    logger,
    memory::{handle_lazy_mmio_fault, handle_read_only_fault, page_4kb_of, MemoryTrackerError},
    power::reboot,
    read_sysreg,
};
use core::fmt::{self, Write};
use core::result;

/// Represents an error that can occur while handling an exception.
#[derive(Debug)]
pub enum HandleExceptionError {
    /// An internal error occurred in the memory tracker.
    InternalError(MemoryTrackerError),
    /// An unknown exception occurred.
    UnknownException,
}

impl From<MemoryTrackerError> for HandleExceptionError {
    fn from(other: MemoryTrackerError) -> Self {
        Self::InternalError(other)
    }
}

impl fmt::Display for HandleExceptionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::InternalError(e) => write!(f, "Error while updating page table: {e}"),
            Self::UnknownException => write!(f, "An unknown exception occurred, not handled."),
        }
    }
}

/// Represents the possible types of exception syndrome register (ESR) values.
#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Esr {
    /// Data abort due to translation fault.
    DataAbortTranslationFault,
    /// Data abort due to permission fault.
    DataAbortPermissionFault,
    /// Data abort due to a synchronous external abort.
    DataAbortSyncExternalAbort,
    /// An unknown ESR value.
    Unknown(usize),
}

impl Esr {
    const EXT_DABT_32BIT: usize = 0x96000010;
    const TRANSL_FAULT_BASE_32BIT: usize = 0x96000004;
    const TRANSL_FAULT_ISS_MASK_32BIT: usize = !0x143;
    const PERM_FAULT_BASE_32BIT: usize = 0x9600004C;
    const PERM_FAULT_ISS_MASK_32BIT: usize = !0x103;
}

impl From<usize> for Esr {
    fn from(esr: usize) -> Self {
        if esr == Self::EXT_DABT_32BIT {
            Self::DataAbortSyncExternalAbort
        } else if esr & Self::TRANSL_FAULT_ISS_MASK_32BIT == Self::TRANSL_FAULT_BASE_32BIT {
            Self::DataAbortTranslationFault
        } else if esr & Self::PERM_FAULT_ISS_MASK_32BIT == Self::PERM_FAULT_BASE_32BIT {
            Self::DataAbortPermissionFault
        } else {
            Self::Unknown(esr)
        }
    }
}

impl fmt::Display for Esr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::DataAbortSyncExternalAbort => write!(f, "Synchronous external abort"),
            Self::DataAbortTranslationFault => write!(f, "Translation fault"),
            Self::DataAbortPermissionFault => write!(f, "Permission fault"),
            Self::Unknown(v) => write!(f, "Unknown exception esr={v:#08x}"),
        }
    }
}

/// A struct representing an Armv8 exception.
pub struct ArmException {
    /// The value of the exception syndrome register.
    pub esr: Esr,
    /// The faulting virtual address read from the fault address register.
    pub far: VirtualAddress,
}

impl fmt::Display for ArmException {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ArmException: esr={}, far={}", self.esr, self.far)
    }
}

impl ArmException {
    /// Reads the values of the EL1 exception syndrome register (`esr_el1`)
    /// and fault address register (`far_el1`) and returns a new instance of
    /// `ArmException` with these values.
    pub fn from_el1_regs() -> Self {
        let esr: Esr = read_sysreg!("esr_el1").into();
        let far = read_sysreg!("far_el1");
        Self { esr, far: VirtualAddress(far) }
    }

    /// Prints the details of an obj and the exception, excluding UART exceptions, and then reboots.
    ///
    /// This uses the emergency console so can safely be called even for synchronous exceptions
    /// without causing a deadlock.
    pub fn print_and_reboot<T: fmt::Display>(&self, exception_name: &str, obj: T, elr: u64) -> ! {
        // Don't print to the UART if we are handling an exception it could raise.
        if !self.is_uart_exception() {
            // SAFETY: We always reboot at the end of this method so there is no way for the
            // original UART driver to be used after this.
            if let Some(mut console) = unsafe { emergency_uart(DEFAULT_EMERGENCY_CONSOLE_INDEX) } {
                // Ignore errors writing to emergency console, as we are about to reboot anyway.
                _ = writeln!(console, "{exception_name}");
                _ = writeln!(console, "{obj}");
                _ = writeln!(console, "{}, elr={:#08x}", self, elr);
            }
        }
        reboot();
    }

    fn is_uart_exception(&self) -> bool {
        self.esr == Esr::DataAbortSyncExternalAbort && page_4kb_of(self.far.0) == UART_PAGE_ADDR
    }
}

fn handle_exception(exception: &ArmException) -> result::Result<(), HandleExceptionError> {
    // Handle all translation faults on both read and write, and MMIO guard map
    // flagged invalid pages or blocks that caused the exception.
    // Handle permission faults for DBM flagged entries, and flag them as dirty on write.
    match exception.esr {
        Esr::DataAbortTranslationFault => handle_lazy_mmio_fault(exception.far)?,
        // TODO(ptosi): Properly filter the ESR for write faults only
        Esr::DataAbortPermissionFault => handle_read_only_fault(exception.far)?,
        _ => return Err(HandleExceptionError::UnknownException),
    }
    Ok(())
}

#[no_mangle]
extern "C" fn sync_exception_current(elr: u64, _spsr: u64) {
    // Disable logging in exception handler to prevent unsafe writes to UART.
    let _guard = logger::suppress();

    let exception = ArmException::from_el1_regs();
    if let Err(e) = handle_exception(&exception) {
        exception.print_and_reboot("sync_exception_current", e, elr);
    }
}

#[no_mangle]
extern "C" fn irq_current(_elr: u64, _spsr: u64) {
    panic!("irq_current");
}

#[no_mangle]
extern "C" fn fiq_current(_elr: u64, _spsr: u64) {
    panic!("fiq_current");
}

#[no_mangle]
extern "C" fn serr_current(_elr: u64, _spsr: u64) {
    let esr = read_sysreg!("esr_el1");
    panic!("serr_current, esr={esr:#08x}");
}

#[no_mangle]
extern "C" fn sync_lower(_elr: u64, _spsr: u64) {
    let esr = read_sysreg!("esr_el1");
    panic!("sync_lower, esr={esr:#08x}");
}

#[no_mangle]
extern "C" fn irq_lower(_elr: u64, _spsr: u64) {
    panic!("irq_lower");
}

#[no_mangle]
extern "C" fn fiq_lower(_elr: u64, _spsr: u64) {
    panic!("fiq_lower");
}

#[no_mangle]
extern "C" fn serr_lower(_elr: u64, _spsr: u64) {
    let esr = read_sysreg!("esr_el1");
    panic!("serr_lower, esr={esr:#08x}");
}
