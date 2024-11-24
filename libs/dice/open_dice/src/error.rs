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
use std::{fmt, result};

#[cfg(feature = "std")]
use std::error::Error;

/// Error type used by DICE.
#[derive(Debug)]
pub enum DiceError {
    /// Provided input was invalid.
    InvalidInput,
    /// Provided buffer was too small.
    BufferTooSmall(usize),
    /// Platform error.
    PlatformError,
    /// Unsupported key algorithm.
    UnsupportedKeyAlgorithm(coset::iana::Algorithm),
}

/// This makes `DiceError` accepted by anyhow.
#[cfg(feature = "std")]
impl Error for DiceError {}

impl fmt::Display for DiceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::InvalidInput => write!(f, "Invalid input"),
            Self::BufferTooSmall(buffer_required_size) => {
                write!(f, "Buffer too small; need {buffer_required_size} bytes")
            }
            Self::PlatformError => write!(f, "Platform error"),
            Self::UnsupportedKeyAlgorithm(algorithm) => {
                write!(f, "Unsupported key algorithm: {algorithm:?}")
            }
        }
    }
}

/// DICE result type.
pub type Result<T> = result::Result<T, DiceError>;

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
