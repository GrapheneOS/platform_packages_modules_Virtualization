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

//! Wrappers around calls to the hypervisor.

pub mod trng;
use self::trng::Error;
use smccc::{
    error::{positive_or_error_64, success_or_error_64},
    hvc64,
};

const ARM_SMCCC_TRNG_VERSION: u32 = 0x8400_0050;
const ARM_SMCCC_TRNG_FEATURES: u32 = 0x8400_0051;
#[allow(dead_code)]
const ARM_SMCCC_TRNG_GET_UUID: u32 = 0x8400_0052;
#[allow(dead_code)]
const ARM_SMCCC_TRNG_RND32: u32 = 0x8400_0053;
pub const ARM_SMCCC_TRNG_RND64: u32 = 0xc400_0053;

/// Returns the (major, minor) version tuple, as defined by the SMCCC TRNG.
pub fn trng_version() -> trng::Result<trng::Version> {
    let args = [0u64; 17];

    let version = positive_or_error_64::<Error>(hvc64(ARM_SMCCC_TRNG_VERSION, args)[0])?;
    (version as u32 as i32).try_into()
}

pub type TrngRng64Entropy = [u64; 3];

pub fn trng_rnd64(nbits: u64) -> trng::Result<TrngRng64Entropy> {
    let mut args = [0u64; 17];
    args[0] = nbits;

    let regs = hvc64(ARM_SMCCC_TRNG_RND64, args);
    success_or_error_64::<Error>(regs[0])?;

    Ok([regs[1], regs[2], regs[3]])
}

pub fn trng_features(fid: u32) -> trng::Result<u64> {
    let mut args = [0u64; 17];
    args[0] = fid as u64;

    positive_or_error_64::<Error>(hvc64(ARM_SMCCC_TRNG_FEATURES, args)[0])
}
