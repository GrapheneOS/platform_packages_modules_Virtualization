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

//! Random number generator implementation for aarch64 platforms using TRNG

use crate::arch::aarch64::hvc;
use crate::rand::{Entropy, Error, Result};
use core::fmt;
use core::mem::size_of;
use smccc::{self, Hvc};
use zerocopy::IntoBytes as _;

/// Error type for rand operations.
pub enum PlatformError {
    /// Error during architectural SMCCC call.
    Smccc(smccc::arch::Error),
    /// Error during SMCCC TRNG call.
    Trng(hvc::trng::Error),
    /// Unsupported SMCCC version.
    UnsupportedSmcccVersion(smccc::arch::Version),
    /// Unsupported SMCCC TRNG version.
    UnsupportedTrngVersion(hvc::trng::Version),
}

impl From<smccc::arch::Error> for Error {
    fn from(e: smccc::arch::Error) -> Self {
        Self::Platform(PlatformError::Smccc(e))
    }
}

impl From<hvc::trng::Error> for Error {
    fn from(e: hvc::trng::Error) -> Self {
        Self::Platform(PlatformError::Trng(e))
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Smccc(e) => write!(f, "Architectural SMCCC error: {e}"),
            Self::Trng(e) => write!(f, "SMCCC TRNG error: {e}"),
            Self::UnsupportedSmcccVersion(v) => write!(f, "Unsupported SMCCC version {v}"),
            Self::UnsupportedTrngVersion(v) => write!(f, "Unsupported SMCCC TRNG version {v}"),
        }
    }
}

impl fmt::Debug for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self}")
    }
}

pub(crate) const MAX_BYTES_PER_CALL: usize = size_of::<u64>() * 3;

/// Configure the source of entropy.
pub(crate) fn init() -> Result<()> {
    // SMCCC TRNG requires SMCCC v1.1.
    match smccc::arch::version::<Hvc>()? {
        smccc::arch::Version { major: 1, minor } if minor >= 1 => (),
        version => return Err(PlatformError::UnsupportedSmcccVersion(version).into()),
    }

    // TRNG_RND requires SMCCC TRNG v1.0.
    match hvc::trng_version()? {
        hvc::trng::Version { major: 1, minor: _ } => (),
        version => return Err(PlatformError::UnsupportedTrngVersion(version).into()),
    }

    // TRNG_RND64 doesn't define any special capabilities so ignore the successful result.
    let _ = hvc::trng_features(hvc::ARM_SMCCC_TRNG_RND64).map_err(|e| {
        if e == hvc::trng::Error::NotSupported {
            // SMCCC TRNG is currently our only source of entropy.
            Error::NoEntropySource
        } else {
            e.into()
        }
    })?;

    Ok(())
}

/// Returns an array where the first `n_bytes` bytes hold entropy.
///
/// The rest of the array should be ignored.
pub(crate) fn platform_entropy(n_bytes: usize) -> Result<Entropy> {
    loop {
        if let Some(entropy) = rnd64(n_bytes)? {
            return Ok(entropy);
        }
    }
}

/// Returns an array where the first `n_bytes` bytes hold entropy, if available.
///
/// The rest of the array should be ignored.
fn rnd64(n_bytes: usize) -> Result<Option<Entropy>> {
    let bits = usize::try_from(u8::BITS).unwrap();
    let result = hvc::trng_rnd64((n_bytes * bits).try_into().unwrap());
    let entropy = if matches!(result, Err(hvc::trng::Error::NoEntropy)) {
        None
    } else {
        let r = result?;
        // From the SMCCC TRNG:
        //
        //     A MAX_BITS-bits wide value (Entropy) is returned across X1 to X3.
        //     The requested conditioned entropy is returned in Entropy[N-1:0].
        //
        //             X1     Entropy[191:128]
        //             X2     Entropy[127:64]
        //             X3     Entropy[63:0]
        //
        //     The bits in Entropy[MAX_BITS-1:N] are 0.
        let reordered = [r[2].to_le(), r[1].to_le(), r[0].to_le()];

        Some(reordered.as_bytes().try_into().unwrap())
    };

    Ok(entropy)
}
