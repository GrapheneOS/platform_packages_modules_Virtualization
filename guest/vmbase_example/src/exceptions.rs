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

use vmbase::{arch::aarch64::exceptions::ArmException, read_sysreg};

#[no_mangle]
extern "C" fn sync_exception_current(elr: u64, _spsr: u64) {
    ArmException::from_el1_regs().print_and_reboot(
        "sync_exception_current",
        "Unexpected synchronous exception",
        elr,
    );
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
    panic!("serr_current, esr={:#08x}", esr);
}

#[no_mangle]
extern "C" fn sync_lower(_elr: u64, _spsr: u64) {
    let esr = read_sysreg!("esr_el1");
    panic!("sync_lower, esr={:#08x}", esr);
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
    panic!("serr_lower, esr={:#08x}", esr);
}
