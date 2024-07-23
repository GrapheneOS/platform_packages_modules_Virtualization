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

//! Exception handlers.

use vmbase::{
    eprintln,
    exceptions::{ArmException, Esr, HandleExceptionError},
    logger,
    memory::{handle_permission_fault, handle_translation_fault},
    power::reboot,
    read_sysreg,
};

fn handle_exception(exception: &ArmException) -> Result<(), HandleExceptionError> {
    // Handle all translation faults on both read and write, and MMIO guard map
    // flagged invalid pages or blocks that caused the exception.
    // Handle permission faults for DBM flagged entries, and flag them as dirty on write.
    match exception.esr {
        Esr::DataAbortTranslationFault => handle_translation_fault(exception.far),
        Esr::DataAbortPermissionFault => handle_permission_fault(exception.far),
        _ => Err(HandleExceptionError::UnknownException),
    }
}

#[no_mangle]
extern "C" fn sync_exception_current(elr: u64, _spsr: u64) {
    // Disable logging in exception handler to prevent unsafe writes to UART.
    let _guard = logger::suppress();

    let exception = ArmException::from_el1_regs();
    if let Err(e) = handle_exception(&exception) {
        exception.print("sync_exception_current", e, elr);
        reboot()
    }
}

#[no_mangle]
extern "C" fn irq_current() {
    eprintln!("irq_current");
    reboot();
}

#[no_mangle]
extern "C" fn fiq_current() {
    eprintln!("fiq_current");
    reboot();
}

#[no_mangle]
extern "C" fn serr_current() {
    eprintln!("serr_current");
    print_esr();
    reboot();
}

#[no_mangle]
extern "C" fn sync_lower() {
    eprintln!("sync_lower");
    print_esr();
    reboot();
}

#[no_mangle]
extern "C" fn irq_lower() {
    eprintln!("irq_lower");
    reboot();
}

#[no_mangle]
extern "C" fn fiq_lower() {
    eprintln!("fiq_lower");
    reboot();
}

#[no_mangle]
extern "C" fn serr_lower() {
    eprintln!("serr_lower");
    print_esr();
    reboot();
}

#[inline]
fn print_esr() {
    let esr = read_sysreg!("esr_el1");
    eprintln!("esr={:#08x}", esr);
}
