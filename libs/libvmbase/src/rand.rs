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

//! Functions and drivers for obtaining true entropy.

use crate::hvc;
use core::fmt;
use core::mem::size_of;
use smccc::{self, Hvc};
use zerocopy::AsBytes as _;

type Entropy = [u8; size_of::<u64>() * 3];

/// Error type for rand operations.
pub enum Error {
    /// No source of entropy found.
    NoEntropySource,
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
        Self::Smccc(e)
    }
}

impl From<hvc::trng::Error> for Error {
    fn from(e: hvc::trng::Error) -> Self {
        Self::Trng(e)
    }
}

/// Result type for rand operations.
pub type Result<T> = core::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NoEntropySource => write!(f, "No source of entropy available"),
            Self::Smccc(e) => write!(f, "Architectural SMCCC error: {e}"),
            Self::Trng(e) => write!(f, "SMCCC TRNG error: {e}"),
            Self::UnsupportedSmcccVersion(v) => write!(f, "Unsupported SMCCC version {v}"),
            Self::UnsupportedTrngVersion(v) => write!(f, "Unsupported SMCCC TRNG version {v}"),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self}")
    }
}

/// Configure the source of entropy.
pub(crate) fn init() -> Result<()> {
    // SMCCC TRNG requires SMCCC v1.1.
    match smccc::arch::version::<Hvc>()? {
        smccc::arch::Version { major: 1, minor } if minor >= 1 => (),
        version => return Err(Error::UnsupportedSmcccVersion(version)),
    }

    // TRNG_RND requires SMCCC TRNG v1.0.
    match hvc::trng_version()? {
        hvc::trng::Version { major: 1, minor: _ } => (),
        version => return Err(Error::UnsupportedTrngVersion(version)),
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

/// Fills a slice of bytes with true entropy.
pub fn fill_with_entropy(s: &mut [u8]) -> Result<()> {
    const MAX_BYTES_PER_CALL: usize = size_of::<Entropy>();

    for chunk in s.chunks_mut(MAX_BYTES_PER_CALL) {
        let entropy = repeat_trng_rnd(chunk.len())?;
        chunk.clone_from_slice(&entropy[..chunk.len()]);
    }

    Ok(())
}

/// Returns an array where the first `n_bytes` bytes hold entropy.
///
/// The rest of the array should be ignored.
fn repeat_trng_rnd(n_bytes: usize) -> Result<Entropy> {
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

/// Generate an array of fixed-size initialized with true-random bytes.
pub fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut arr = [0; N];
    fill_with_entropy(&mut arr)?;
    Ok(arr)
}
