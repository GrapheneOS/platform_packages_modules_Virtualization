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

//! Random number generator implementation for x86_64

use crate::rand::{Entropy, Error, Result};
use core::arch::asm;
use core::arch::x86_64::__cpuid_count;
use core::fmt;

const RDSEED_MAX_RETRIES: i32 = 10;

/// Error type for rand operations.
pub enum PlatformError {
    /// CPU does not have the necessary random number generator instruction.
    UnsupportedCpu,
    /// Hardware random number generator is not ready yet.
    NoEntropy,
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PlatformError::UnsupportedCpu => write!(f, "unsupported CPU"),
            PlatformError::NoEntropy => write!(f, "no entropy"),
        }
    }
}

pub(crate) const MAX_BYTES_PER_CALL: usize = size_of::<u64>();

fn rdseed_supported() -> bool {
    // SAFETY: CPUID itself and CPUID leaf EAX=7,ECX=0 are available on all x86-64 implementations
    // and has no memory-safety side effects.
    let extended_features = unsafe { __cpuid_count(7, 0) };
    extended_features.ebx & (1 << 18) != 0
}

pub(crate) fn init() -> Result<()> {
    if rdseed_supported() {
        Ok(())
    } else {
        Err(Error::Platform(PlatformError::UnsupportedCpu))
    }
}

/// Returns an array where the first `n_bytes` bytes hold entropy.
///
/// The rest of the array should be ignored.
pub(crate) fn platform_entropy(n_bytes: usize) -> Result<Entropy> {
    assert!(n_bytes <= MAX_BYTES_PER_CALL);

    for _ in 0..RDSEED_MAX_RETRIES {
        if let Some(random_data) = rdseed64() {
            return Ok(random_data.to_ne_bytes());
        }
    }

    Err(Error::Platform(PlatformError::NoEntropy))
}

// Return 64 bits of random data from the RDSEED instruction, or `None` if the entropy source is not
// ready yet.
fn rdseed64() -> Option<u64> {
    let random_data: u64;
    let carry_flag: u8;

    // SAFETY: RDSEED is available at all privilege levels and only writes to the output register
    // and EFLAGS.
    unsafe {
        asm!(
            "rdseed {}",
            "setc {}",
            out(reg) random_data,
            out(reg_byte) carry_flag,
            options(nomem, nostack),
        );
    }

    if carry_flag == 1 {
        Some(random_data)
    } else {
        None
    }
}
