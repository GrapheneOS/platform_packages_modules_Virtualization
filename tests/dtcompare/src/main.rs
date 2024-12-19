// Copyright 2024 The Android Open Source Project
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

//! Compare device tree contents.
//! Allows skipping over fields provided.

use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use hex::encode;
use libfdt::Fdt;
use libfdt::FdtNode;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs::read;
use std::path::PathBuf;

#[derive(Debug, Parser)]
/// Device Tree Compare arguments.
struct DtCompareArgs {
    /// first device tree
    #[arg(long)]
    dt1: PathBuf,
    /// second device tree
    #[arg(long)]
    dt2: PathBuf,
    /// list of properties that should exist but are expected to hold different values in the
    /// trees.
    #[arg(short = 'I', long)]
    ignore_path_value: Vec<String>,
    /// list of paths that will be ignored, whether added, removed, or changed.
    /// Paths can be nodes, subnodes, or even properties:
    /// Ex: /avf/unstrusted // this is a path to a subnode. All properties and subnodes underneath
    ///                     // it will also be ignored.
    ///     /avf/name       // This is a path for a property. Only this property will be ignored.
    #[arg(short = 'S', long)]
    ignore_path: Vec<String>,
}

fn main() -> Result<()> {
    let args = DtCompareArgs::parse();
    let dt1: Vec<u8> = read(args.dt1)?;
    let dt2: Vec<u8> = read(args.dt2)?;
    let ignore_value_set = BTreeSet::from_iter(args.ignore_path_value);
    let ignore_set = BTreeSet::from_iter(args.ignore_path);
    compare_device_trees(dt1.as_slice(), dt2.as_slice(), ignore_value_set, ignore_set)
}

// Compare device trees by doing a pre-order traversal of the trees.
fn compare_device_trees(
    dt1: &[u8],
    dt2: &[u8],
    ignore_value_set: BTreeSet<String>,
    ignore_set: BTreeSet<String>,
) -> Result<()> {
    let fdt1 = Fdt::from_slice(dt1).context("invalid device tree: Dt1")?;
    let fdt2 = Fdt::from_slice(dt2).context("invalid device tree: Dt2")?;
    let mut errors = Vec::new();
    compare_subnodes(
        &fdt1.root(),
        &fdt2.root(),
        &ignore_value_set,
        &ignore_set,
        /* path */ &mut ["".to_string()],
        &mut errors,
    )?;
    if !errors.is_empty() {
        return Err(anyhow!(
            "Following properties had different values: [\n{}\n]\ndetected {} diffs",
            errors.join("\n"),
            errors.len()
        ));
    }
    Ok(())
}

fn compare_props(
    root1: &FdtNode,
    root2: &FdtNode,
    ignore_value_set: &BTreeSet<String>,
    ignore_set: &BTreeSet<String>,
    path: &mut [String],
    errors: &mut Vec<String>,
) -> Result<()> {
    let mut prop_map: BTreeMap<String, &[u8]> = BTreeMap::new();
    for prop in root1.properties().context("Error getting properties")? {
        let prop_path =
            path.join("/") + "/" + prop.name().context("Error getting property name")?.to_str()?;
        // Do not add to prop map if skipping
        if ignore_set.contains(&prop_path) {
            continue;
        }
        let value = prop.value().context("Error getting value")?;
        if prop_map.insert(prop_path.clone(), value).is_some() {
            return Err(anyhow!("Duplicate property detected in subnode: {}", prop_path));
        }
    }
    for prop in root2.properties().context("Error getting properties")? {
        let prop_path =
            path.join("/") + "/" + prop.name().context("Error getting property name")?.to_str()?;
        if ignore_set.contains(&prop_path) {
            continue;
        }
        let Some(prop1_value) = prop_map.remove(&prop_path) else {
            errors.push(format!("added prop_path: {}", prop_path));
            continue;
        };
        let prop_compare = prop1_value == prop.value().context("Error getting value")?;
        // Check if value should be ignored. If yes, skip field.
        if ignore_value_set.contains(&prop_path) {
            continue;
        }
        if !prop_compare {
            errors.push(format!(
                "prop {} value mismatch: old: {} -> new: {}",
                prop_path,
                encode(prop1_value),
                encode(prop.value().context("Error getting value")?)
            ));
        }
    }
    if !prop_map.is_empty() {
        errors.push(format!("missing properties: {:?}", prop_map));
    }
    Ok(())
}

fn compare_subnodes(
    node1: &FdtNode,
    node2: &FdtNode,
    ignore_value_set: &BTreeSet<String>,
    ignore_set: &BTreeSet<String>,
    path: &mut [String],
    errors: &mut Vec<String>,
) -> Result<()> {
    let mut subnodes_map: BTreeMap<String, FdtNode> = BTreeMap::new();
    for subnode in node1.subnodes().context("Error getting subnodes of first FDT")? {
        let sn_path = path.join("/")
            + "/"
            + subnode.name().context("Error getting property name")?.to_str()?;
        // Do not add to subnode map if skipping
        if ignore_set.contains(&sn_path) {
            continue;
        }
        if subnodes_map.insert(sn_path.clone(), subnode).is_some() {
            return Err(anyhow!("Duplicate subnodes detected: {}", sn_path));
        }
    }
    for sn2 in node2.subnodes().context("Error getting subnodes of second FDT")? {
        let sn_path =
            path.join("/") + "/" + sn2.name().context("Error getting subnode name")?.to_str()?;
        let sn1 = subnodes_map.remove(&sn_path);
        match sn1 {
            Some(sn) => {
                compare_props(
                    &sn,
                    &sn2,
                    ignore_value_set,
                    ignore_set,
                    &mut [sn_path.clone()],
                    errors,
                )?;
                compare_subnodes(
                    &sn,
                    &sn2,
                    ignore_value_set,
                    ignore_set,
                    &mut [sn_path.clone()],
                    errors,
                )?;
            }
            None => errors.push(format!("added node: {}", sn_path)),
        }
    }
    if !subnodes_map.is_empty() {
        errors.push(format!("missing nodes: {:?}", subnodes_map));
    }
    Ok(())
}
