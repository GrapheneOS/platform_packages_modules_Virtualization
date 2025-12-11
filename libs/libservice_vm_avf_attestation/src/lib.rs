// Copyright 2026, The Android Open Source Project
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

//! This library contains Service VM specific implementation for `avf_attestation`.

#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use avf_attestation::{
    AttestationOps, ClientVmDiceChain, DiceChainEntryPayload, InMemoryKeyDerivationOps,
    KeyDerivationOps,
};
use bssl_avf::Digester;
use coset::{CoseSign1, CoseSign1Builder, HeaderBuilder};
use diced_open_dice::{derive_cdi_leaf_priv, sign, DiceArtifacts, PrivateKey, VM_KEY_ALGORITHM};
use log::{debug, error, info};
use service_vm_comm::{RequestProcessingError, Result};
use zeroize::Zeroizing;

const DICE_CHAIN_SUFFIX_LEN: usize = 1;

/// The Service VM implementation of attestation operations.
pub struct Ops<'a> {
    dice_artifacts: &'a (dyn DiceArtifacts + Sync),
    vendor_hashtree_root_digest: Option<&'a [u8]>,
    key_derivation_ops: InMemoryKeyDerivationOps,
}

impl<'a> Ops<'a> {
    /// Creates a new instance of `Ops`.
    pub fn new(
        dice_artifacts: &'a (dyn DiceArtifacts + Sync),
        vendor_hashtree_root_digest: Option<&'a [u8]>,
    ) -> Result<Self> {
        let key_derivation_ops = Zeroizing::new(dice_artifacts.cdi_seal().to_vec()).into();
        Ok(Self { dice_artifacts, vendor_hashtree_root_digest, key_derivation_ops })
    }

    fn validate_vendor_module_code_hash(
        &self,
        vendor_module_cert: &DiceChainEntryPayload,
    ) -> Result<()> {
        let Some(expected_root_digest) = self.vendor_hashtree_root_digest else {
            error!(
                "The vendor partition is present in the DICE chain, \
                but the vendor_hashtree_root_digest is not provided in the DT"
            );
            return Err(RequestProcessingError::NoVendorHashTreeRootDigestInDT);
        };
        if Digester::sha512().digest(expected_root_digest)? == vendor_module_cert.code_hash {
            Ok(())
        } else {
            error!(
                "The vendor partition code hash in the Client VM DICE chain does \
                not match the expected value from the DT"
            );
            Err(RequestProcessingError::InvalidVendorPartition)
        }
    }
}

impl<'a> KeyDerivationOps for Ops<'a> {
    fn derive_kek(&self, salt: &[u8], info: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        self.key_derivation_ops.derive_kek(salt, info)
    }
}

impl<'a> AttestationOps for Ops<'a> {
    fn reference_vm_dice_chain(&self) -> Result<&[u8]> {
        self.dice_artifacts.bcc().ok_or(RequestProcessingError::MissingDiceChain)
    }

    fn reference_vm_dice_chain_suffix_len(&self) -> usize {
        DICE_CHAIN_SUFFIX_LEN
    }

    fn sign_with_cdi_leaf(&self, payload: &[u8]) -> Result<CoseSign1> {
        let cdi_leaf_priv = derive_cdi_leaf_priv(None, self.dice_artifacts).map_err(|e| {
            error!("Failed to derive the CDI_Leaf_Priv: {e}");
            RequestProcessingError::InternalError
        })?;
        let protected = HeaderBuilder::new().algorithm(VM_KEY_ALGORITHM.into()).build();
        let signed_data = CoseSign1Builder::new()
            .protected(protected)
            .payload(payload.to_vec())
            .try_create_signature(&[], |message| sign_message(message, &cdi_leaf_priv))?
            .build();
        Ok(signed_data)
    }

    fn validate_vm(&self, client_chain_suffix: &ClientVmDiceChain) -> service_vm_comm::Result<()> {
        if let Some(vendor_module_cert) = client_chain_suffix.vendor_module() {
            info!("Vendor partition present in the Client VM DICE chain.");
            self.validate_vendor_module_code_hash(vendor_module_cert)?;
        } else {
            debug!("No vendor partition present in the Client VM DICE chain.");
        }
        Ok(())
    }
}

fn sign_message(message: &[u8], private_key: &PrivateKey) -> Result<Vec<u8>> {
    Ok(sign(message, private_key.as_array())
        .map_err(|e| {
            error!("Failed to sign the CSR: {e}");
            RequestProcessingError::InternalError
        })?
        .to_vec())
}
