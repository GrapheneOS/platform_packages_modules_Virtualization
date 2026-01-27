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

//! This module handles the pvmfw payload verification.

use crate::ops::PvmfwAvbVerifier;
use crate::partition::PartitionName;
use crate::PvmfwVerifyError;
use alloc::{string::String, vec::Vec};
use avb::{
    Descriptor, DescriptorError, DescriptorResult, HashDescriptor, SlotVerifyError,
    SlotVerifyNoDataResult, VbmetaData,
};
use core::str;

// We use this for the rollback_index field if SlotVerifyData has empty rollback_indexes
const DEFAULT_ROLLBACK_INDEX: u64 = 0;

/// SHA256 digest length
pub const DIGEST_LEN: usize = 32;

/// SHA256 digest type for kernel and initrd.
pub type Digest = [u8; DIGEST_LEN];

/// Verified data returned when the payload verification succeeds.
#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedBootData<'a> {
    /// DebugLevel of the VM.
    pub debug_level: DebugLevel,
    /// Kernel digest.
    pub kernel_digest: Digest,
    /// Initrd digest if initrd exists.
    pub initrd_digest: Option<Digest>,
    /// VBMeta digest.
    pub vbmeta_digest: Digest,
    /// Trusted public key.
    pub public_key: &'a [u8],
    /// VM capabilities.
    pub capabilities: Vec<Capability>,
    /// Rollback index of kernel.
    pub rollback_index: u64,
    /// Page size of kernel, if present.
    pub page_size: Option<usize>,
    /// Name of the guest payload, if present.
    pub name: Option<String>,
}

impl VerifiedBootData<'_> {
    /// Name of the Remote Key Provisioning VM.
    pub const RKP_VM_NAME: &'static str = "rkp_vm";
    /// Name of the Trusty-based TEE VM for desktop platforms.
    pub const DESKTOP_TRUSTY_VM_NAME: &'static str = "desktop-trusty";

    /// Returns whether the kernel have the given capability
    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }
}

/// This enum corresponds to the `DebugLevel` in `VirtualMachineConfig`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugLevel {
    /// Not debuggable at all.
    None,
    /// Fully debuggable.
    Full,
}

/// VM Capability.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Capability {
    /// Remote attestation.
    RemoteAttest,
    /// Secretkeeper protected secrets.
    SecretkeeperProtection,
    /// Trusty security VM.
    TrustySecurityVm,
    /// UEFI support for booting guest kernel.
    SupportsUefiBoot,
    /// (internal)
    #[allow(non_camel_case_types)] // TODO: Use mem::variant_count once stable.
    _VARIANT_COUNT,
}

impl Capability {
    const KEY: &'static str = "com.android.virt.cap";
    const REMOTE_ATTEST: &'static [u8] = b"remote_attest";
    const TRUSTY_SECURITY_VM: &'static [u8] = b"trusty_security_vm";
    const SECRETKEEPER_PROTECTION: &'static [u8] = b"secretkeeper_protection";
    const SEPARATOR: u8 = b'|';
    const SUPPORTS_UEFI_BOOT: &'static [u8] = b"supports_uefi_boot";
    /// Number of supported capabilites.
    pub const COUNT: usize = Self::_VARIANT_COUNT as usize;

    /// Returns the capabilities indicated in `descriptor`, or error if the descriptor has
    /// unexpected contents.
    fn get_capabilities(vbmeta_data: &VbmetaData) -> Result<Vec<Self>, PvmfwVerifyError> {
        let Some(value) = vbmeta_data.get_property_value(Self::KEY) else {
            return Ok(Vec::new());
        };

        let mut res = Vec::new();

        for v in value.split(|b| *b == Self::SEPARATOR) {
            let cap = match v {
                Self::REMOTE_ATTEST => Self::RemoteAttest,
                Self::TRUSTY_SECURITY_VM => Self::TrustySecurityVm,
                Self::SECRETKEEPER_PROTECTION => Self::SecretkeeperProtection,
                Self::SUPPORTS_UEFI_BOOT => Self::SupportsUefiBoot,
                _ => return Err(PvmfwVerifyError::UnknownVbmetaProperty),
            };
            if res.contains(&cap) {
                return Err(SlotVerifyError::InvalidMetadata.into());
            }
            res.push(cap);
        }
        Ok(res)
    }
}

/// Hash descriptors extracted from a vbmeta image.
///
/// We always have a kernel hash descriptor and may have initrd normal or debug descriptors.
struct HashDescriptors<'a> {
    kernel: &'a HashDescriptor<'a>,
    initrd_normal: Option<&'a HashDescriptor<'a>>,
    initrd_debug: Option<&'a HashDescriptor<'a>>,
}

impl<'a> HashDescriptors<'a> {
    /// Extracts the hash descriptors from all vbmeta descriptors. Any unexpected hash descriptor
    /// is an error.
    fn get(descriptors: &'a [Descriptor<'a>]) -> DescriptorResult<Self> {
        let mut kernel = None;
        let mut initrd_normal = None;
        let mut initrd_debug = None;

        for descriptor in descriptors.iter().filter_map(|d| match d {
            Descriptor::Hash(h) => Some(h),
            _ => None,
        }) {
            let target = match descriptor
                .partition_name
                .as_bytes()
                .try_into()
                .map_err(|_| DescriptorError::InvalidContents)?
            {
                PartitionName::Kernel => &mut kernel,
                PartitionName::InitrdNormal => &mut initrd_normal,
                PartitionName::InitrdDebug => &mut initrd_debug,
            };

            if target.is_some() {
                // Duplicates of the same partition name is an error.
                return Err(DescriptorError::InvalidContents);
            }
            target.replace(descriptor);
        }

        // Kernel is required, the others are optional.
        Ok(Self {
            kernel: kernel.ok_or(DescriptorError::InvalidContents)?,
            initrd_normal,
            initrd_debug,
        })
    }

    /// Returns an error if either initrd descriptor exists.
    fn verify_no_initrd(&self) -> Result<(), PvmfwVerifyError> {
        match self.initrd_normal.or(self.initrd_debug) {
            Some(_) => Err(SlotVerifyError::InvalidMetadata.into()),
            None => Ok(()),
        }
    }
}

/// Returns a copy of the SHA256 digest in `descriptor`, or error if the sizes don't match.
fn copy_digest(descriptor: &HashDescriptor) -> SlotVerifyNoDataResult<Digest> {
    let mut digest = Digest::default();
    if descriptor.digest.len() != digest.len() {
        return Err(SlotVerifyError::InvalidMetadata);
    }
    digest.clone_from_slice(descriptor.digest);
    Ok(digest)
}

/// Returns the indicated payload page size, if present.
fn read_page_size(vbmeta_data: &VbmetaData) -> Result<Option<usize>, PvmfwVerifyError> {
    let Some(property) = vbmeta_data.get_property_value("com.android.virt.page_size") else {
        return Ok(None);
    };
    let size = str::from_utf8(property)
        .or(Err(PvmfwVerifyError::InvalidPageSize))?
        .parse::<usize>()
        .or(Err(PvmfwVerifyError::InvalidPageSize))?
        .checked_mul(1024)
        // TODO(stable(unsigned_is_multiple_of)): use .is_multiple_of()
        .filter(|sz| sz % (4 << 10) == 0 && *sz != 0)
        .ok_or(PvmfwVerifyError::InvalidPageSize)?;

    Ok(Some(size))
}

/// Returns the indicated payload name, if present.
fn read_name(vbmeta_data: &VbmetaData) -> Result<Option<String>, PvmfwVerifyError> {
    let Some(property) = vbmeta_data.get_property_value("com.android.virt.name") else {
        return Ok(None);
    };
    let name = str::from_utf8(property).map_err(|_| PvmfwVerifyError::InvalidVmName)?;
    if name.is_empty() {
        return Err(PvmfwVerifyError::InvalidVmName);
    }
    Ok(Some(name.into()))
}

/// Verifies the payload (signed kernel + initrd) against the trusted public key.
pub fn verify_payload<'a>(
    kernel: &[u8],
    initrd: Option<&[u8]>,
    trusted_public_key: &'a [u8],
) -> Result<VerifiedBootData<'a>, PvmfwVerifyError> {
    let mut verifier = PvmfwAvbVerifier::new(kernel, initrd, trusted_public_key);
    let kernel_verify_result = verifier.verify_partition(PartitionName::Kernel)?;

    // TODO(b/302093437): Use explicit rollback_index_location instead of default
    // location (first element).
    let rollback_index =
        *kernel_verify_result.rollback_indexes().first().unwrap_or(&DEFAULT_ROLLBACK_INDEX);
    let vbmeta_image = kernel_verify_result.vbmeta_data().first().unwrap();
    let descriptors = vbmeta_image.descriptors()?;
    let hash_descriptors = HashDescriptors::get(&descriptors)?;
    let capabilities = Capability::get_capabilities(vbmeta_image)?;
    let page_size = read_page_size(vbmeta_image)?;
    let name = read_name(vbmeta_image)?;
    let vbmeta_digest = kernel_verify_result.calculate_sha256_digest();

    if initrd.is_none() {
        hash_descriptors.verify_no_initrd()?;
        return Ok(VerifiedBootData {
            debug_level: DebugLevel::None,
            kernel_digest: copy_digest(hash_descriptors.kernel)?,
            initrd_digest: None,
            vbmeta_digest,
            public_key: verifier.get_validated_vbmeta_key().unwrap(),
            capabilities,
            rollback_index,
            page_size,
            name,
        });
    }

    let initrd = initrd.unwrap();
    let size = initrd.len();
    let (debug_level, initrd_descriptor) =
        if verifier.verify_sized_partition(PartitionName::InitrdNormal, size).is_ok() {
            (DebugLevel::None, hash_descriptors.initrd_normal)
        } else if verifier.verify_sized_partition(PartitionName::InitrdDebug, size).is_ok() {
            (DebugLevel::Full, hash_descriptors.initrd_debug)
        } else {
            return Err(SlotVerifyError::Verification(None).into());
        };
    let initrd_descriptor = initrd_descriptor.ok_or(DescriptorError::InvalidContents)?;
    Ok(VerifiedBootData {
        debug_level,
        kernel_digest: copy_digest(hash_descriptors.kernel)?,
        initrd_digest: Some(copy_digest(initrd_descriptor)?),
        public_key: verifier.get_validated_vbmeta_key().unwrap(),
        vbmeta_digest,
        capabilities,
        rollback_index,
        page_size,
        name,
    })
}
