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

//! Errors and relating functions thrown in this library.

use open_dice_cbor_bindgen::DiceResult;

/// Error type used by DICE.
#[derive(Debug, thiserror::Error)]
pub enum DiceError {
    /// Provided input was invalid.
    #[error("Invalid input")]
    InvalidInput,
    /// Provided buffer was too small.
    #[error("Buffer too small; need {0} bytes")]
    BufferTooSmall(usize),
    /// Platform error.
    #[error("Platform error")]
    PlatformError,
    /// Unsupported key algorithm.
    #[error("Unsupported key algorithm: {0:?}")]
    UnsupportedKeyAlgorithm(coset::iana::Algorithm),
    /// A failed fallible allocation. Used in no_std environments.
    #[error("Memory allocation failed")]
    MemoryAllocationError,
    /// DICE chain not found in artifacts.
    #[error("DICE chain not found in artifacts")]
    DiceChainNotFound,
}

/// DICE result type.
pub type Result<T> = std::result::Result<T, DiceError>;

/// Checks the given `DiceResult`. Returns an error if it's not OK.
pub(crate) fn check_result(result: DiceResult, buffer_required_size: usize) -> Result<()> {
    match result {
        DiceResult::kDiceResultOk => Ok(()),
        DiceResult::kDiceResultInvalidInput => Err(DiceError::InvalidInput),
        DiceResult::kDiceResultBufferTooSmall => {
            Err(DiceError::BufferTooSmall(buffer_required_size))
        }
        DiceResult::kDiceResultPlatformError => Err(DiceError::PlatformError),
    }
}
