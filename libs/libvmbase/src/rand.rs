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

use crate::arch::rand::{platform_entropy, PlatformError, MAX_BYTES_PER_CALL};

pub(crate) type Entropy = [u8; MAX_BYTES_PER_CALL];

/// Error type for rand operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No source of entropy found.
    #[error("No source of entropy available")]
    NoEntropySource,

    /// Platform specific error
    #[error("Platform error: {0}")]
    Platform(#[from] PlatformError),
}

/// Result type for rand operations.
pub type Result<T> = core::result::Result<T, Error>;

/// Fills a slice of bytes with true entropy.
pub fn fill_with_entropy(s: &mut [u8]) -> Result<()> {
    for chunk in s.chunks_mut(MAX_BYTES_PER_CALL) {
        let entropy = platform_entropy(chunk.len())?;
        chunk.clone_from_slice(&entropy[..chunk.len()]);
    }

    Ok(())
}

/// Generate an array of fixed-size initialized with true-random bytes.
pub fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut arr = [0; N];
    fill_with_entropy(&mut arr)?;
    Ok(arr)
}
