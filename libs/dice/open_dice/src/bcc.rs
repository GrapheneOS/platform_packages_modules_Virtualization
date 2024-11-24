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

//! This module mirrors the content in open-dice/include/dice/android.h

use crate::dice::{context, Cdi, CdiValues, DiceArtifacts, InputValues, CDI_SIZE};
use crate::error::{check_result, DiceError, Result};
use open_dice_android_bindgen::{
    DiceAndroidConfigValues, DiceAndroidFormatConfigDescriptor, DiceAndroidHandoverParse,
    DiceAndroidMainFlow, DICE_ANDROID_CONFIG_COMPONENT_NAME, DICE_ANDROID_CONFIG_COMPONENT_VERSION,
    DICE_ANDROID_CONFIG_RESETTABLE, DICE_ANDROID_CONFIG_RKP_VM_MARKER,
    DICE_ANDROID_CONFIG_SECURITY_VERSION,
};
use std::{ffi::CStr, ptr};

/// Contains the input values used to construct the Android Profile for DICE
/// configuration descriptor.
#[derive(Default, Debug)]
pub struct DiceConfigValues<'a> {
    /// Name of the component.
    pub component_name: Option<&'a CStr>,
    /// Version of the component.
    pub component_version: Option<u64>,
    /// Whether the key changes on factory reset.
    pub resettable: bool,
    /// Monotonically increasing version of the component.
    pub security_version: Option<u64>,
    /// Whether the component can take part in running the RKP VM.
    pub rkp_vm_marker: bool,
}

/// Formats a configuration descriptor following the Android Profile for DICE specification.
/// See https://pigweed.googlesource.com/open-dice/+/refs/heads/main/docs/android.md.
pub fn bcc_format_config_descriptor(values: &DiceConfigValues, buffer: &mut [u8]) -> Result<usize> {
    let mut configs = 0;

    let component_name = values.component_name.map_or(ptr::null(), |name| {
        configs |= DICE_ANDROID_CONFIG_COMPONENT_NAME;
        name.as_ptr()
    });
    let component_version = values.component_version.map_or(0, |version| {
        configs |= DICE_ANDROID_CONFIG_COMPONENT_VERSION;
        version
    });
    if values.resettable {
        configs |= DICE_ANDROID_CONFIG_RESETTABLE;
    }
    let security_version = values.security_version.map_or(0, |version| {
        configs |= DICE_ANDROID_CONFIG_SECURITY_VERSION;
        version
    });
    if values.rkp_vm_marker {
        configs |= DICE_ANDROID_CONFIG_RKP_VM_MARKER;
    }

    let values =
        DiceAndroidConfigValues { configs, component_name, component_version, security_version };

    let mut buffer_size = 0;
    check_result(
        // SAFETY: The function writes to the buffer, within the given bounds, and only reads the
        // input values. It writes its result to buffer_size.
        unsafe {
            DiceAndroidFormatConfigDescriptor(
                &values,
                buffer.len(),
                buffer.as_mut_ptr(),
                &mut buffer_size,
            )
        },
        buffer_size,
    )?;
    Ok(buffer_size)
}

/// Executes the main Android DICE flow.
///
/// Given a full set of input values along with the current DICE chain and CDI values,
/// computes the next CDI values and matching updated DICE chain.
pub fn bcc_main_flow(
    current_cdi_attest: &Cdi,
    current_cdi_seal: &Cdi,
    current_chain: &[u8],
    input_values: &InputValues,
    next_cdi_values: &mut CdiValues,
    next_chain: &mut [u8],
) -> Result<usize> {
    let mut next_chain_size = 0;
    check_result(
        // SAFETY: `DiceAndroidMainFlow` only reads the `current_chain` and CDI values and writes
        // to `next_chain` and next CDI values within its bounds. It also reads `input_values` as a
        // constant input and doesn't store any pointer.
        // The first argument is a pointer to a valid |DiceContext_| object for multi-alg open-dice
        // and a null pointer otherwise.
        unsafe {
            DiceAndroidMainFlow(
                context(),
                current_cdi_attest.as_ptr(),
                current_cdi_seal.as_ptr(),
                current_chain.as_ptr(),
                current_chain.len(),
                input_values.as_ptr(),
                next_chain.len(),
                next_chain.as_mut_ptr(),
                &mut next_chain_size,
                next_cdi_values.cdi_attest.as_mut_ptr(),
                next_cdi_values.cdi_seal.as_mut_ptr(),
            )
        },
        next_chain_size,
    )?;
    Ok(next_chain_size)
}

/// Executes the main Android DICE handover flow.
///
/// A handover combines the DICE chain and CDIs in a single CBOR object.
/// This function takes the current boot stage's handover bundle and produces a
/// bundle for the next stage.
#[cfg(feature = "multialg")]
pub fn bcc_handover_main_flow(
    current_handover: &[u8],
    input_values: &InputValues,
    next_handover: &mut [u8],
    ctx: crate::dice::DiceContext,
) -> Result<usize> {
    let mut next_handover_size = 0;
    let mut ctx: open_dice_cbor_bindgen::DiceContext_ = ctx.into();
    check_result(
        // SAFETY: The function only reads `current_handover` and writes to `next_handover`
        // within its bounds,
        // It also reads `input_values` as a constant input and doesn't store any pointer.
        // The first argument is a pointer to a valid |DiceContext_| object.
        unsafe {
            open_dice_android_bindgen::DiceAndroidHandoverMainFlow(
                &mut ctx as *mut _ as *mut std::ffi::c_void,
                current_handover.as_ptr(),
                current_handover.len(),
                input_values.as_ptr(),
                next_handover.len(),
                next_handover.as_mut_ptr(),
                &mut next_handover_size,
            )
        },
        next_handover_size,
    )?;

    Ok(next_handover_size)
}

/// An Android DICE handover object combines the DICE chain and CDIs in a single CBOR object.
/// This struct is used as return of the function `android_dice_handover_parse`, its lifetime is
/// tied to the lifetime of the raw handover slice.
#[derive(Debug)]
pub struct BccHandover<'a> {
    /// Attestation CDI.
    cdi_attest: &'a [u8; CDI_SIZE],
    /// Sealing CDI.
    cdi_seal: &'a [u8; CDI_SIZE],
    /// DICE chain.
    bcc: Option<&'a [u8]>,
}

impl<'a> DiceArtifacts for BccHandover<'a> {
    fn cdi_attest(&self) -> &[u8; CDI_SIZE] {
        self.cdi_attest
    }

    fn cdi_seal(&self) -> &[u8; CDI_SIZE] {
        self.cdi_seal
    }

    fn bcc(&self) -> Option<&[u8]> {
        self.bcc
    }
}

/// This function parses the `handover` to extracts the DICE chain and CDIs.
/// The lifetime of the returned `DiceAndroidHandover` is tied to the given `handover` slice.
pub fn bcc_handover_parse(handover: &[u8]) -> Result<BccHandover> {
    let mut cdi_attest: *const u8 = ptr::null();
    let mut cdi_seal: *const u8 = ptr::null();
    let mut chain: *const u8 = ptr::null();
    let mut chain_size = 0;
    check_result(
        // SAFETY: The `handover` is only read and never stored and the returned pointers should
        // all point within the address range of the `handover` or be NULL.
        unsafe {
            DiceAndroidHandoverParse(
                handover.as_ptr(),
                handover.len(),
                &mut cdi_attest,
                &mut cdi_seal,
                &mut chain,
                &mut chain_size,
            )
        },
        chain_size,
    )?;
    let cdi_attest = sub_slice(handover, cdi_attest, CDI_SIZE)?;
    let cdi_seal = sub_slice(handover, cdi_seal, CDI_SIZE)?;
    let bcc = sub_slice(handover, chain, chain_size).ok();
    Ok(BccHandover {
        cdi_attest: cdi_attest.try_into().map_err(|_| DiceError::PlatformError)?,
        cdi_seal: cdi_seal.try_into().map_err(|_| DiceError::PlatformError)?,
        bcc,
    })
}

/// Gets a slice the `addr` points to and of length `len`.
/// The slice should be contained in the buffer.
fn sub_slice(buffer: &[u8], addr: *const u8, len: usize) -> Result<&[u8]> {
    if addr.is_null() || !buffer.as_ptr_range().contains(&addr) {
        return Err(DiceError::PlatformError);
    }
    // SAFETY: This is safe because addr is not null and is within the range of the buffer.
    let start: usize = unsafe {
        addr.offset_from(buffer.as_ptr()).try_into().map_err(|_| DiceError::PlatformError)?
    };
    start.checked_add(len).and_then(|end| buffer.get(start..end)).ok_or(DiceError::PlatformError)
}
