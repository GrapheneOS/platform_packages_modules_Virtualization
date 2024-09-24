// Copyright 2024, The Android Open Source Project
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

//! This module support creating AFV related overlays, that can then be appended to DT by VM.

use anyhow::{anyhow, Result};
use cstr::cstr;
use fsfdt::FsFdt;
use libfdt::{Fdt, FdtError};
use std::ffi::CStr;
use std::path::Path;

pub(crate) const AVF_NODE_NAME: &CStr = cstr!("avf");
pub(crate) const UNTRUSTED_NODE_NAME: &CStr = cstr!("untrusted");
pub(crate) const VM_DT_OVERLAY_PATH: &str = "vm_dt_overlay.dtbo";
pub(crate) const VM_DT_OVERLAY_MAX_SIZE: usize = 2000;

/// Create a Device tree overlay containing the provided proc style device tree & properties!
/// # Arguments
/// * `dt_path` - (Optional) Path to (proc style) device tree to be included in the overlay.
/// * `untrusted_props` - Include a property in /avf/untrusted node. This node is used to specify
///   host provided properties such as `instance-id`.
/// * `trusted_props` - Include a property in /avf node. This overwrites nodes included with
///   `dt_path`. In pVM, pvmfw will reject if it doesn't match the value in pvmfw config.
///
/// Example: with `create_device_tree_overlay(_, _, [("instance-id", _),], [("digest", _),])`
/// ```
///   {
///     fragment@0 {
///         target-path = "/";
///         __overlay__ {
///             avf {
///                 digest = [ 0xaa 0xbb .. ]
///                 untrusted { instance-id = [ 0x01 0x23 .. ] }
///               }
///             };
///         };
///     };
/// };
/// ```
pub(crate) fn create_device_tree_overlay<'a>(
    buffer: &'a mut [u8],
    dt_path: Option<&'a Path>,
    untrusted_props: &[(&'a CStr, &'a [u8])],
    trusted_props: &[(&'a CStr, &'a [u8])],
) -> Result<&'a mut Fdt> {
    if dt_path.is_none() && untrusted_props.is_empty() && trusted_props.is_empty() {
        return Err(anyhow!("Expected at least one device tree addition"));
    }

    let fdt =
        Fdt::create_empty_tree(buffer).map_err(|e| anyhow!("Failed to create empty Fdt: {e:?}"))?;
    let mut fragment = fdt
        .root_mut()
        .add_subnode(cstr!("fragment@0"))
        .map_err(|e| anyhow!("Failed to add fragment node: {e:?}"))?;
    fragment
        .setprop(cstr!("target-path"), b"/\0")
        .map_err(|e| anyhow!("Failed to set target-path property: {e:?}"))?;
    let overlay = fragment
        .add_subnode(cstr!("__overlay__"))
        .map_err(|e| anyhow!("Failed to add __overlay__ node: {e:?}"))?;
    let avf =
        overlay.add_subnode(AVF_NODE_NAME).map_err(|e| anyhow!("Failed to add avf node: {e:?}"))?;

    if !untrusted_props.is_empty() {
        let mut untrusted = avf
            .add_subnode(UNTRUSTED_NODE_NAME)
            .map_err(|e| anyhow!("Failed to add untrusted node: {e:?}"))?;
        for (name, value) in untrusted_props {
            untrusted
                .setprop(name, value)
                .map_err(|e| anyhow!("Failed to set untrusted property: {e:?}"))?;
        }
    }

    // Read dt_path from host DT and overlay onto fdt.
    if let Some(path) = dt_path {
        fdt.overlay_onto(cstr!("/fragment@0/__overlay__"), path)?;
    }

    if cfg!(tpu_assignable_device) {
        let mut avf = fdt
            .node_mut(cstr!("/fragment@0/__overlay__/avf"))
            .map_err(|e| anyhow!("Failed to search avf node: {e:?}"))?
            .ok_or(anyhow!("Failed to get avf node"))?;
        let vendor_digest = cstr!("vendor_hashtree_descriptor_root_digest");
        // Remove the vendor digest.
        // In the case it is actually requested, it will be re-added by virtue of being in
        // `trusted_props`.
        match avf.delprop(vendor_digest) {
            Ok(()) | Err(FdtError::NotFound) => {}
            Err(e) => {
                return Err(anyhow!("Unexpected error pre-removing {vendor_digest:?}: {e:?}"))
            }
        }
        for (name, value) in trusted_props {
            avf.setprop(name, value)
                .map_err(|e| anyhow!("Failed to set trusted property: {e:?}"))?;
        }
    } else if !trusted_props.is_empty() {
        let mut avf = fdt
            .node_mut(cstr!("/fragment@0/__overlay__/avf"))
            .map_err(|e| anyhow!("Failed to search avf node: {e:?}"))?
            .ok_or(anyhow!("Failed to get avf node"))?;
        for (name, value) in trusted_props {
            avf.setprop(name, value)
                .map_err(|e| anyhow!("Failed to set trusted property: {e:?}"))?;
        }
    }

    fdt.pack().map_err(|e| anyhow!("Failed to pack DT overlay, {e:?}"))?;

    Ok(fdt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_overlays_not_allowed() {
        let mut buffer = vec![0_u8; VM_DT_OVERLAY_MAX_SIZE];
        let res = create_device_tree_overlay(&mut buffer, None, &[], &[]);
        assert!(res.is_err());
    }

    #[test]
    fn untrusted_prop_test() {
        let mut buffer = vec![0_u8; VM_DT_OVERLAY_MAX_SIZE];
        let prop_name = cstr!("XOXO");
        let prop_val_input = b"OXOX";
        let fdt =
            create_device_tree_overlay(&mut buffer, None, &[(prop_name, prop_val_input)], &[])
                .unwrap();

        let prop_value_dt = fdt
            .node(cstr!("/fragment@0/__overlay__/avf/untrusted"))
            .unwrap()
            .expect("/avf/untrusted node doesn't exist")
            .getprop(prop_name)
            .unwrap()
            .expect("Prop not found!");
        assert_eq!(prop_value_dt, prop_val_input, "Unexpected property value");
    }

    #[test]
    fn trusted_prop_test() {
        let mut buffer = vec![0_u8; VM_DT_OVERLAY_MAX_SIZE];
        let prop_name = cstr!("XOXOXO");
        let prop_val_input = b"OXOXOX";
        let fdt =
            create_device_tree_overlay(&mut buffer, None, &[], &[(prop_name, prop_val_input)])
                .unwrap();

        let prop_value_dt = fdt
            .node(cstr!("/fragment@0/__overlay__/avf"))
            .unwrap()
            .expect("/avf node doesn't exist")
            .getprop(prop_name)
            .unwrap()
            .expect("Prop not found!");
        assert_eq!(prop_value_dt, prop_val_input, "Unexpected property value");
    }
}
