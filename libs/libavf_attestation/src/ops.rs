/*
 * Copyright (C) 2026 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Defines the abstract operations for VM Attestation and Key Provisioning.

use crate::dice::ClientVmDiceChain;
use alloc::vec::Vec;
use coset::CoseSign1;
use service_vm_comm::Result;
use zeroize::Zeroizing;

/// Abstraction for hardware-backed key derivation.
pub trait KeyDerivationOps: Send + Sync {
    /// Derives a Key Encryption Key (KEK) from a hardware-backed secret using a KDF.
    ///
    /// The derived KEK is used to wrap (encrypt) locally generated private key material
    /// (e.g., the remotely provisioned keyblob).
    ///
    /// # Arguments
    /// * `salt` - A non-secret value used to randomize the derivation.
    /// * `info` - Application-specific context information (e.g., "KeyBlobEncryption").
    fn derive_kek(&self, salt: &[u8], info: &[u8]) -> Result<Zeroizing<[u8; 32]>>;
}

/// A provider for platform-specific cryptographic and validation operations required
/// for VM attestation.
///
/// This trait abstracts the differences between Attesters while providing a unified
/// interface for the core attestation logic.
pub trait AttestationOps: KeyDerivationOps + Send + Sync {
    /// Returns the full DICE chain of the reference VM (the Attester).
    fn reference_vm_dice_chain(&self) -> Result<&[u8]>;

    /// Returns the length of the suffix in the reference DICE chain that is specific
    /// to the Attester VM itself.
    ///
    /// The "Common Chain Prefix" is calculated as: `len(reference_chain) - suffix_len`.
    fn reference_vm_dice_chain_suffix_len(&self) -> usize;

    /// Signs the provided payload using the DICE Chain's Leaf Private Key (CDI_Leaf_Priv).
    ///
    /// This is primarily used to generate the Certificate Signing Request (CSR) sent to the
    /// RKP server.
    fn sign_with_cdi_leaf(&self, payload: &[u8]) -> Result<CoseSign1>;

    /// Validates a Client VM's DICE chain against platform policies.
    ///
    /// This method delegates the specific validation logic (e.g., Allow-Listing,
    /// Anti-Rollback) to the platform integrator.
    fn validate_vm(&self, client_chain_suffix: &ClientVmDiceChain) -> Result<()>;

    /// Returns the UDS certificates.
    ///
    /// # Returns
    /// A Vec<u8> containing the CBOR-encoded UdsCerts map, as defined in the
    /// IRemotelyProvisionedComponent HAL CDDL:
    ///
    /// ```text
    /// UdsCerts = {
    ///     * SignerName => UdsCertChain
    /// }
    /// ```
    fn uds_certs(&self) -> Result<Vec<u8>>;
}
