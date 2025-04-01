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
use core::fmt;

/// Error type for rand operations.
pub struct PlatformError;

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "rand error")
    }
}

pub(crate) const MAX_BYTES_PER_CALL: usize = 1;

pub(crate) fn init() -> Result<()> {
    Ok(())
}

/// Returns an array where the first `n_bytes` bytes hold entropy.
///
/// The rest of the array should be ignored.
pub(crate) fn platform_entropy(n_bytes: usize) -> Result<Entropy> {
    assert_eq!(n_bytes, MAX_BYTES_PER_CALL);
    // TODO(b/375569109): Provide a proper implementation
    log::warn!("Platform source of the entropy is not yet implemented!");
    Ok([42])
}
