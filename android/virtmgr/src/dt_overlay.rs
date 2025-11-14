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
use fsfdt::FsFdt;
use libfdt::{Fdt, FdtNodeMut};
use std::ffi::CStr;
use std::path::Path;

pub(crate) const VM_DT_OVERLAY_PATH: &str = "vm_dt_overlay.dtbo";
pub(crate) const VM_DT_OVERLAY_MAX_SIZE: usize = 5000;

/// Create an empty device tree overlay
pub(crate) fn create_empty_device_tree_overlay(buffer: &mut [u8]) -> Result<&mut Fdt> {
    let fdt =
        Fdt::create_empty_tree(buffer).map_err(|e| anyhow!("Failed to create empty Fdt: {e:?}"))?;
    let mut fragment = fdt
        .root_mut()
        .add_subnode(c"fragment@0")
        .map_err(|e| anyhow!("Failed to add fragment node: {e:?}"))?;
    fragment
        .setprop(c"target-path", b"/\0")
        .map_err(|e| anyhow!("Failed to set target-path property: {e:?}"))?;
    let _ = fragment
        .add_subnode(c"__overlay__")
        .map_err(|e| anyhow!("Failed to add __overlay__ node: {e:?}"))?;
    Ok(fdt)
}

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
    android_firmware_props: &[(&'a CStr, &'a [u8])],
) -> Result<&'a mut Fdt> {
    if dt_path.is_none()
        && untrusted_props.is_empty()
        && trusted_props.is_empty()
        && android_firmware_props.is_empty()
    {
        return Err(anyhow!("Expected at least one device tree addition"));
    }

    let fdt =
        Fdt::create_empty_tree(buffer).map_err(|e| anyhow!("Failed to create empty Fdt: {e:?}"))?;

    if !untrusted_props.is_empty() {
        let path = c"/fragment@0/__overlay__/avf/untrusted";
        let mut node = fdt
            .find_or_add_node_mut(path)
            .map_err(|e| anyhow!("Failed to add node '{path:?}': {e:?}"))?;
        add_props_to_node(untrusted_props, &mut node)?;
    }

    // Read dt_path from host DT and overlay onto fdt.
    if let Some(path) = dt_path {
        fdt.overlay_onto(c"/fragment@0/__overlay__", path)?;
    }

    if !trusted_props.is_empty() {
        let path = c"/fragment@0/__overlay__/avf";
        let mut node = fdt
            .find_or_add_node_mut(path)
            .map_err(|e| anyhow!("Failed to add node '{path:?}': {e:?}"))?;
        add_props_to_node(trusted_props, &mut node)?;
    }

    if !android_firmware_props.is_empty() {
        let path = c"/fragment@0/__overlay__/firmware/android";
        let mut node = fdt
            .find_or_add_node_mut(path)
            .map_err(|e| anyhow!("Failed to add node '{path:?}': {e:?}"))?;
        add_props_to_node(android_firmware_props, &mut node)?;
        node.setprop(c"compatible", b"android,firmware\0")
            .map_err(|e| anyhow!("Failed to set compatible property: {e:?}"))?;
    }

    if let Some(mut node) = fdt.node_mut(c"/fragment@0")? {
        node.setprop(c"target-path", b"/\0")
            .map_err(|e| anyhow!("Failed to set target-path property: {e:?}"))?;
    }

    fdt.pack().map_err(|e| anyhow!("Failed to pack DT overlay, {e:?}"))?;

    Ok(fdt)
}

fn add_props_to_node<'a>(props: &[(&'a CStr, &'a [u8])], node: &mut FdtNodeMut) -> Result<()> {
    for (name, value) in props {
        node.setprop(name, value).map_err(|e| {
            let node_name = node.as_node().name();
            anyhow!("Failed to set '{node_name:?}' property '{name:?}': {e:?}")
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_overlays_not_allowed() {
        let mut buffer = vec![0_u8; VM_DT_OVERLAY_MAX_SIZE];
        let res = create_device_tree_overlay(&mut buffer, None, &[], &[], &[]);
        assert!(res.is_err());
    }

    #[test]
    fn untrusted_prop_test() {
        let mut buffer = vec![0_u8; VM_DT_OVERLAY_MAX_SIZE];
        let prop_name = c"XOXO";
        let prop_val_input = b"OXOX";
        let fdt =
            create_device_tree_overlay(&mut buffer, None, &[(prop_name, prop_val_input)], &[], &[])
                .unwrap();

        let prop_value_dt = fdt
            .node(c"/fragment@0/__overlay__/avf/untrusted")
            .unwrap()
            .expect("/avf/untrusted node doesn't exist")
            .getprop(prop_name)
            .unwrap()
            .expect("Prop not found!");
        assert_eq!(prop_value_dt, prop_val_input, "Unexpected property value");
        assert_eq!(fdt.node(c"/fragment@0/__overlay__/firmware"), Ok(None));
    }

    #[test]
    fn trusted_prop_test() {
        let mut buffer = vec![0_u8; VM_DT_OVERLAY_MAX_SIZE];
        let prop_name = c"XOXOXO";
        let prop_val_input = b"OXOXOX";
        let fdt =
            create_device_tree_overlay(&mut buffer, None, &[], &[(prop_name, prop_val_input)], &[])
                .unwrap();

        let prop_value_dt = fdt
            .node(c"/fragment@0/__overlay__/avf")
            .unwrap()
            .expect("/avf node doesn't exist")
            .getprop(prop_name)
            .unwrap()
            .expect("Prop not found!");
        assert_eq!(prop_value_dt, prop_val_input, "Unexpected property value");
        assert_eq!(fdt.node(c"/fragment@0/__overlay__/firmware"), Ok(None));
    }

    #[test]
    fn firmware_prop_test() {
        let mut buffer = vec![0_u8; VM_DT_OVERLAY_MAX_SIZE];
        let prop_name = c"XOXOXO";
        let prop_val_input = b"OXOXOX";
        let firmware_props = [
            (c"vbmeta.device_state", c"locked".to_bytes_with_nul()),
            (c"vbmeta.digest", c"00000000000000000000000000000000".to_bytes_with_nul()),
            (
                c"vbmeta.public_key_digest",
                c"0000000000000000000000000000000000000000000000000000000000000000"
                    .to_bytes_with_nul(),
            ),
            (c"verifiedbootstate", c"fake".to_bytes_with_nul()),
        ];
        let fdt = create_device_tree_overlay(
            &mut buffer,
            None,
            &[],
            &[(prop_name, prop_val_input)],
            &firmware_props,
        )
        .unwrap();

        let prop_value_dt = fdt
            .node(c"/fragment@0/__overlay__/avf")
            .unwrap()
            .expect("/avf node doesn't exist")
            .getprop(prop_name)
            .unwrap()
            .expect("Prop not found!");
        assert_eq!(prop_value_dt, prop_val_input, "Unexpected property value");

        for (prop_name, prop_value) in firmware_props {
            let prop_value_dt = fdt
                .node(c"/fragment@0/__overlay__/firmware/android")
                .unwrap()
                .expect("/firmware/android node doesn't exist")
                .getprop(prop_name)
                .unwrap()
                .expect("Prop not found!");
            assert_eq!(prop_value_dt, prop_value, "Unexpected property value");
        }
    }

    #[test]
    fn firmware_prop_only_test() {
        let mut buffer = vec![0_u8; VM_DT_OVERLAY_MAX_SIZE];
        let firmware_props = [
            (c"vbmeta.device_state", c"locked".to_bytes_with_nul()),
            (c"vbmeta.digest", c"00000000000000000000000000000000".to_bytes_with_nul()),
            (
                c"vbmeta.public_key_digest",
                c"0000000000000000000000000000000000000000000000000000000000000000"
                    .to_bytes_with_nul(),
            ),
            (c"verifiedbootstate", c"fake".to_bytes_with_nul()),
        ];
        let fdt = create_device_tree_overlay(&mut buffer, None, &[], &[], &firmware_props).unwrap();

        assert_eq!(fdt.node(c"/fragment@0/__overlay__/avf"), Ok(None));
        assert!(fdt
            .node(c"/fragment@0")
            .unwrap()
            .expect("/fragment@0 node doesn't exist")
            .getprop(c"target-path")
            .unwrap()
            .is_some());

        for (prop_name, prop_value) in firmware_props {
            let prop_value_dt = fdt
                .node(c"/fragment@0/__overlay__/firmware/android")
                .unwrap()
                .expect("/firmware/android node doesn't exist")
                .getprop(prop_name)
                .unwrap()
                .expect("Prop not found!");
            assert_eq!(prop_value_dt, prop_value, "Unexpected property value");
        }
    }
}
