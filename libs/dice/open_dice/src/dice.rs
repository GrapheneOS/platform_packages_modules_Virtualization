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

//! Structs and functions about the types used in DICE.
//! This module mirrors the content in open-dice/include/dice/dice.h

use crate::error::{check_result, Result};
pub use open_dice_cbor_bindgen::DiceMode;
use open_dice_cbor_bindgen::{
    DiceConfigType, DiceDeriveCdiCertificateId, DiceDeriveCdiPrivateKeySeed, DiceInputValues,
    DiceMainFlow, DICE_CDI_SIZE, DICE_HASH_SIZE, DICE_HIDDEN_SIZE, DICE_ID_SIZE,
    DICE_INLINE_CONFIG_SIZE, DICE_PRIVATE_KEY_SEED_SIZE, DICE_PRIVATE_KEY_SIZE,
    DICE_PUBLIC_KEY_SIZE, DICE_SIGNATURE_SIZE,
};
#[cfg(feature = "serde_derive")]
use serde_derive::{Deserialize, Serialize};
use std::{marker::PhantomData, ptr};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// The size of a DICE hash.
pub const HASH_SIZE: usize = DICE_HASH_SIZE as usize;
/// The size of the DICE hidden value.
pub const HIDDEN_SIZE: usize = DICE_HIDDEN_SIZE as usize;
/// The size of a DICE inline config.
const INLINE_CONFIG_SIZE: usize = DICE_INLINE_CONFIG_SIZE as usize;
/// The size of a CDI.
pub const CDI_SIZE: usize = DICE_CDI_SIZE as usize;
/// The size of a private key seed.
pub const PRIVATE_KEY_SEED_SIZE: usize = DICE_PRIVATE_KEY_SEED_SIZE as usize;
/// The size of a private key.
pub const PRIVATE_KEY_SIZE: usize = DICE_PRIVATE_KEY_SIZE as usize;
/// The size of a public key.
pub const PUBLIC_KEY_SIZE: usize = DICE_PUBLIC_KEY_SIZE as usize;
/// The size of a signature.
pub const SIGNATURE_SIZE: usize = DICE_SIGNATURE_SIZE as usize;
/// The size of an ID.
pub const ID_SIZE: usize = DICE_ID_SIZE as usize;

/// Array type of hashes used by DICE.
pub type Hash = [u8; HASH_SIZE];
/// Array type of additional input.
pub type Hidden = [u8; HIDDEN_SIZE];
/// Array type of inline configuration values.
pub type InlineConfig = [u8; INLINE_CONFIG_SIZE];
/// Array type of CDIs.
pub type Cdi = [u8; CDI_SIZE];
/// Array type of the public key.
pub type PublicKey = [u8; PUBLIC_KEY_SIZE];
/// Array type of the signature.
pub type Signature = [u8; SIGNATURE_SIZE];
/// Array type of DICE ID.
pub type DiceId = [u8; ID_SIZE];

/// A trait for types that represent Dice artifacts, which include:
///
/// - Attestation CDI
/// - Sealing CDI
/// - Boot Certificate Chain
///
/// Types that implement this trait provide an access these artifacts.
pub trait DiceArtifacts {
    /// Returns a reference to the attestation CDI.
    fn cdi_attest(&self) -> &[u8; CDI_SIZE];

    /// Returns a reference to the sealing CDI.
    fn cdi_seal(&self) -> &[u8; CDI_SIZE];

    /// Returns a reference to the Boot Certificate Chain, if present.
    fn bcc(&self) -> Option<&[u8]>;
}

/// TODO(b/268587826): Clean up the memory cache after zeroing out the memory
/// for sensitive data like CDI values and private key.
/// CDI Values.
#[derive(Debug, Zeroize, ZeroizeOnDrop, Default)]
#[cfg_attr(feature = "serde_derive", derive(Serialize, Deserialize))]
pub struct CdiValues {
    /// Attestation CDI.
    pub cdi_attest: [u8; CDI_SIZE],
    /// Sealing CDI.
    pub cdi_seal: [u8; CDI_SIZE],
}

/// Private key seed. The data is zeroed out when the struct is dropped.
#[derive(Zeroize, ZeroizeOnDrop, Default)]
pub struct PrivateKeySeed([u8; PRIVATE_KEY_SEED_SIZE]);

impl PrivateKeySeed {
    /// Returns an array reference of the private key seed.
    pub fn as_array(&self) -> &[u8; PRIVATE_KEY_SEED_SIZE] {
        &self.0
    }

    /// Returns a mutable pointer to the slice buffer of the private key seed.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.0.as_mut_ptr()
    }
}

/// Private key. The data is zeroed out when the struct is dropped.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PrivateKey([u8; PRIVATE_KEY_SIZE]);

impl Default for PrivateKey {
    /// Creates a new `PrivateKey` instance with all bytes set to 0.
    ///
    /// Since the size of the private key array is too large to be initialized
    /// with a default value, this implementation sets all the bytes in the array
    /// to 0 using the `[0u8; PRIVATE_KEY_SIZE]` syntax.
    fn default() -> Self {
        Self([0u8; PRIVATE_KEY_SIZE])
    }
}

impl PrivateKey {
    /// Returns an array reference of the private key.
    pub fn as_array(&self) -> &[u8; PRIVATE_KEY_SIZE] {
        &self.0
    }

    /// Returns a mutable pointer to the slice buffer of the private key.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.0.as_mut_ptr()
    }
}

/// Configuration descriptor for DICE input values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Config<'a> {
    /// Reference to an inline descriptor.
    Inline(&'a InlineConfig),
    /// Reference to a free form descriptor that will be hashed by the implementation.
    Descriptor(&'a [u8]),
}

impl Config<'_> {
    fn dice_config_type(&self) -> DiceConfigType {
        match self {
            Self::Inline(_) => DiceConfigType::kDiceConfigTypeInline,
            Self::Descriptor(_) => DiceConfigType::kDiceConfigTypeDescriptor,
        }
    }

    fn inline_config(&self) -> InlineConfig {
        match self {
            Self::Inline(inline) => **inline,
            Self::Descriptor(_) => [0u8; INLINE_CONFIG_SIZE],
        }
    }

    fn descriptor_ptr(&self) -> *const u8 {
        match self {
            Self::Descriptor(descriptor) => descriptor.as_ptr(),
            _ => ptr::null(),
        }
    }

    fn descriptor_size(&self) -> usize {
        match self {
            Self::Descriptor(descriptor) => descriptor.len(),
            _ => 0,
        }
    }
}

/// Wrap of `DiceInputValues`.
#[derive(Clone, Debug)]
pub struct InputValues<'a> {
    dice_inputs: DiceInputValues,
    // DiceInputValues contains a pointer to the separate config descriptor, which must therefore
    // outlive it. Make sure the borrow checker can enforce that.
    config_descriptor: PhantomData<&'a [u8]>,
}

impl<'a> InputValues<'a> {
    /// Creates a new `InputValues`.
    pub fn new(
        code_hash: Hash,
        config: Config<'a>,
        authority_hash: Hash,
        mode: DiceMode,
        hidden: Hidden,
    ) -> Self {
        Self {
            dice_inputs: DiceInputValues {
                code_hash,
                code_descriptor: ptr::null(),
                code_descriptor_size: 0,
                config_type: config.dice_config_type(),
                config_value: config.inline_config(),
                config_descriptor: config.descriptor_ptr(),
                config_descriptor_size: config.descriptor_size(),
                authority_hash,
                authority_descriptor: ptr::null(),
                authority_descriptor_size: 0,
                mode,
                hidden,
            },
            config_descriptor: PhantomData,
        }
    }

    /// Returns a raw pointer to the wrapped `DiceInputValues`.
    pub fn as_ptr(&self) -> *const DiceInputValues {
        &self.dice_inputs as *const DiceInputValues
    }
}

/// Derives a CDI private key seed from a `cdi_attest` value.
pub fn derive_cdi_private_key_seed(cdi_attest: &Cdi) -> Result<PrivateKeySeed> {
    let mut seed = PrivateKeySeed::default();
    check_result(
        // SAFETY: The function writes to the buffer within the given bounds, and only reads the
        // input values. The first argument context is not used in this function.
        unsafe {
            DiceDeriveCdiPrivateKeySeed(
                ptr::null_mut(), // context
                cdi_attest.as_ptr(),
                seed.as_mut_ptr(),
            )
        },
        seed.0.len(),
    )?;
    Ok(seed)
}

/// Derives an ID from the given `cdi_public_key` value.
pub fn derive_cdi_certificate_id(cdi_public_key: &[u8]) -> Result<DiceId> {
    let mut id = [0u8; ID_SIZE];
    check_result(
        // SAFETY: The function writes to the buffer within the given bounds, and only reads the
        // input values. The first argument context is not used in this function.
        unsafe {
            DiceDeriveCdiCertificateId(
                ptr::null_mut(), // context
                cdi_public_key.as_ptr(),
                cdi_public_key.len(),
                id.as_mut_ptr(),
            )
        },
        id.len(),
    )?;
    Ok(id)
}

/// Executes the main DICE flow.
///
/// Given a full set of input values and the current CDI values, computes the
/// next CDI values and a matching certificate.
/// Returns the actual size of the next CDI certificate.
pub fn dice_main_flow(
    current_cdi_attest: &Cdi,
    current_cdi_seal: &Cdi,
    input_values: &InputValues,
    next_cdi_certificate: &mut [u8],
    next_cdi_values: &mut CdiValues,
) -> Result<usize> {
    let mut next_cdi_certificate_actual_size = 0;
    check_result(
        // SAFETY: The function only reads the current CDI values and inputs and writes
        // to `next_cdi_certificate` and next CDI values within its bounds.
        // The first argument can be null and is not used in the current implementation.
        unsafe {
            DiceMainFlow(
                ptr::null_mut(), // context
                current_cdi_attest.as_ptr(),
                current_cdi_seal.as_ptr(),
                input_values.as_ptr(),
                next_cdi_certificate.len(),
                next_cdi_certificate.as_mut_ptr(),
                &mut next_cdi_certificate_actual_size,
                next_cdi_values.cdi_attest.as_mut_ptr(),
                next_cdi_values.cdi_seal.as_mut_ptr(),
            )
        },
        next_cdi_certificate_actual_size,
    )?;
    Ok(next_cdi_certificate_actual_size)
}
