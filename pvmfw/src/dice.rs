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

//! Support for DICE derivation and BCC generation.

use core::mem::size_of;
use cstr::cstr;
use diced_open_dice::{
    bcc_format_config_descriptor, bcc_handover_main_flow, hash, Config, DiceConfigValues, DiceMode,
    Hash, InputValues, HIDDEN_SIZE,
};
use pvmfw_avb::{Capability, DebugLevel, Digest, VerifiedBootData};
use zerocopy::AsBytes;

fn to_dice_mode(debug_level: DebugLevel) -> DiceMode {
    match debug_level {
        DebugLevel::None => DiceMode::kDiceModeNormal,
        DebugLevel::Full => DiceMode::kDiceModeDebug,
    }
}

fn to_dice_hash(verified_boot_data: &VerifiedBootData) -> diced_open_dice::Result<Hash> {
    let mut digests = [0u8; size_of::<Digest>() * 2];
    digests[..size_of::<Digest>()].copy_from_slice(&verified_boot_data.kernel_digest);
    if let Some(initrd_digest) = verified_boot_data.initrd_digest {
        digests[size_of::<Digest>()..].copy_from_slice(&initrd_digest);
    }
    hash(&digests)
}

pub struct PartialInputs {
    pub code_hash: Hash,
    pub auth_hash: Hash,
    pub mode: DiceMode,
    pub security_version: u64,
    pub rkp_vm_marker: bool,
}

impl PartialInputs {
    pub fn new(data: &VerifiedBootData) -> diced_open_dice::Result<Self> {
        let code_hash = to_dice_hash(data)?;
        let auth_hash = hash(data.public_key)?;
        let mode = to_dice_mode(data.debug_level);
        // We use rollback_index from vbmeta as the security_version field in dice certificate.
        let security_version = data.rollback_index;
        let rkp_vm_marker = data.has_capability(Capability::RemoteAttest);

        Ok(Self { code_hash, auth_hash, mode, security_version, rkp_vm_marker })
    }

    pub fn write_next_bcc(
        self,
        current_bcc_handover: &[u8],
        salt: &[u8; HIDDEN_SIZE],
        next_bcc: &mut [u8],
    ) -> diced_open_dice::Result<()> {
        let mut config_descriptor_buffer = [0; 128];
        let config = self.generate_config_descriptor(&mut config_descriptor_buffer)?;

        let dice_inputs = InputValues::new(
            self.code_hash,
            Config::Descriptor(config),
            self.auth_hash,
            self.mode,
            self.make_hidden(salt)?,
        );
        let _ = bcc_handover_main_flow(current_bcc_handover, &dice_inputs, next_bcc)?;
        Ok(())
    }

    fn make_hidden(&self, salt: &[u8; HIDDEN_SIZE]) -> diced_open_dice::Result<[u8; HIDDEN_SIZE]> {
        // We want to make sure we get a different sealing CDI for:
        // - VMs with different salt values
        // - An RKP VM and any other VM (regardless of salt)
        // The hidden input for DICE affects the sealing CDI (but the values in the config
        // descriptor do not).
        // Since the hidden input has to be a fixed size, create it as a hash of the values we
        // want included.
        #[derive(AsBytes)]
        #[repr(C, packed)]
        struct HiddenInput {
            rkp_vm_marker: bool,
            salt: [u8; HIDDEN_SIZE],
        }
        // TODO(b/291213394): Include `defer_rollback_protection` flag in the Hidden Input to
        // differentiate the secrets in both cases.
        hash(HiddenInput { rkp_vm_marker: self.rkp_vm_marker, salt: *salt }.as_bytes())
    }

    fn generate_config_descriptor<'a>(
        &self,
        config_descriptor_buffer: &'a mut [u8],
    ) -> diced_open_dice::Result<&'a [u8]> {
        let config_values = DiceConfigValues {
            component_name: Some(cstr!("vm_entry")),
            security_version: if cfg!(dice_changes) { Some(self.security_version) } else { None },
            rkp_vm_marker: self.rkp_vm_marker,
            ..Default::default()
        };
        let config_descriptor_size =
            bcc_format_config_descriptor(&config_values, config_descriptor_buffer)?;
        let config = &config_descriptor_buffer[..config_descriptor_size];
        Ok(config)
    }
}

/// Flushes data caches over the provided address range.
///
/// # Safety
///
/// The provided address and size must be to an address range that is valid for read and write
/// (typically on the stack, .bss, .data, or provided BCC) from a single allocation
/// (e.g. stack array).
#[no_mangle]
#[cfg(not(test))]
unsafe extern "C" fn DiceClearMemory(
    _ctx: *mut core::ffi::c_void,
    size: usize,
    addr: *mut core::ffi::c_void,
) {
    use core::slice;
    use vmbase::memory::flushed_zeroize;

    // SAFETY: We require our caller to provide a valid range within a single object. The open-dice
    // always calls this on individual stack-allocated arrays which ensures that.
    let region = unsafe { slice::from_raw_parts_mut(addr as *mut u8, size) };
    flushed_zeroize(region)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ciborium::Value;
    use std::collections::HashMap;
    use std::vec;

    const COMPONENT_NAME_KEY: i64 = -70002;
    const COMPONENT_VERSION_KEY: i64 = -70003;
    const RESETTABLE_KEY: i64 = -70004;
    const SECURITY_VERSION_KEY: i64 = -70005;
    const RKP_VM_MARKER_KEY: i64 = -70006;

    const BASE_VB_DATA: VerifiedBootData = VerifiedBootData {
        debug_level: DebugLevel::None,
        kernel_digest: [1u8; size_of::<Digest>()],
        initrd_digest: Some([2u8; size_of::<Digest>()]),
        public_key: b"public key",
        capabilities: vec![],
        rollback_index: 42,
    };

    #[test]
    fn base_data_conversion() {
        let vb_data = BASE_VB_DATA;
        let inputs = PartialInputs::new(&vb_data).unwrap();

        assert_eq!(inputs.mode, DiceMode::kDiceModeNormal);
        assert_eq!(inputs.security_version, 42);
        assert!(!inputs.rkp_vm_marker);

        // TODO(b/313608219): Consider checks for code_hash and possibly auth_hash.
    }

    #[test]
    fn debuggable_conversion() {
        let vb_data = VerifiedBootData { debug_level: DebugLevel::Full, ..BASE_VB_DATA };
        let inputs = PartialInputs::new(&vb_data).unwrap();

        assert_eq!(inputs.mode, DiceMode::kDiceModeDebug);
    }

    #[test]
    fn rkp_vm_conversion() {
        let vb_data =
            VerifiedBootData { capabilities: vec![Capability::RemoteAttest], ..BASE_VB_DATA };
        let inputs = PartialInputs::new(&vb_data).unwrap();

        assert!(inputs.rkp_vm_marker);
    }

    #[test]
    fn base_config_descriptor() {
        let vb_data = BASE_VB_DATA;
        let inputs = PartialInputs::new(&vb_data).unwrap();
        let config_map = decode_config_descriptor(&inputs);

        assert_eq!(config_map.get(&COMPONENT_NAME_KEY).unwrap().as_text().unwrap(), "vm_entry");
        assert_eq!(config_map.get(&COMPONENT_VERSION_KEY), None);
        assert_eq!(config_map.get(&RESETTABLE_KEY), None);
        if cfg!(dice_changes) {
            assert_eq!(
                config_map.get(&SECURITY_VERSION_KEY).unwrap().as_integer().unwrap(),
                42.into()
            );
        } else {
            assert_eq!(config_map.get(&SECURITY_VERSION_KEY), None);
        }
        assert_eq!(config_map.get(&RKP_VM_MARKER_KEY), None);
    }

    #[test]
    fn config_descriptor_with_rkp_vm() {
        let vb_data =
            VerifiedBootData { capabilities: vec![Capability::RemoteAttest], ..BASE_VB_DATA };
        let inputs = PartialInputs::new(&vb_data).unwrap();
        let config_map = decode_config_descriptor(&inputs);

        assert!(config_map.get(&RKP_VM_MARKER_KEY).unwrap().is_null());
    }

    fn decode_config_descriptor(inputs: &PartialInputs) -> HashMap<i64, Value> {
        let mut buffer = [0; 128];
        let config_descriptor = inputs.generate_config_descriptor(&mut buffer).unwrap();

        let cbor_map =
            cbor_util::deserialize::<Value>(config_descriptor).unwrap().into_map().unwrap();

        cbor_map
            .into_iter()
            .map(|(k, v)| ((k.into_integer().unwrap().try_into().unwrap()), v))
            .collect()
    }
}
