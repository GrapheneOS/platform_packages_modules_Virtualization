// Copyright 2025, The Android Open Source Project
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

//! Tenant Manager
//! Manages the tenants and their attributes running inside Microdroid.

use super::instance::{ApexData, ApkData, InstanceDisk, InstanceSpec};
use crate::MicrodroidError;
use anyhow::{anyhow, bail, Context, Result};
use log::{info, warn};
use microdroid_payload_config::TenantConfig;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TenantPackageInfo {
    ApkData(ApkData),
    ApexData(ApexData),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub(crate) struct TenantAttribute {
    uid: u32,
}

impl TenantAttribute {
    pub fn uid(&self) -> u32 {
        self.uid
    }

    pub fn gid() -> u32 {
        microdroid_uids::MICRODROID_PAYLOAD_GID
    }
}

#[derive(Debug)]
pub(crate) struct TenantManager {
    tenants: HashMap<String, TenantAttribute>,
    next_tenant_uid: u32,
}

impl TenantManager {
    fn new() -> Self {
        Self {
            // TODO(basantwani): Add persistence by integrating with InstanceSpec
            tenants: HashMap::new(),
            next_tenant_uid: microdroid_uids::MICRODROID_FIRST_TENANT_UID,
        }
    }

    pub fn initialize(tenants_config: &[TenantConfig]) -> Result<Self> {
        let mut manager = Self::new();
        for tenant in tenants_config {
            let name = match tenant {
                TenantConfig::Apex(c) => &c.name,
                TenantConfig::Apk(c) => &c.name,
            };
            manager.register_tenant_package(name)?;
        }
        Ok(manager)
    }

    pub fn register_tenant_package(&mut self, package_name: &str) -> Result<()> {
        if self.tenants.contains_key(package_name) {
            warn!("Tenant already registered: {package_name}");
            return Ok(());
        }

        let uid = self.next_tenant_uid;
        self.next_tenant_uid += 1;

        let attribute = TenantAttribute { uid };
        info!("Registering tenant: {package_name} with uid: {:?}", uid);
        self.tenants.insert(package_name.to_string(), attribute);
        // TODO(basantwani): update instance spec
        Ok(())
    }

    pub fn get_tenant_attribute(&self, package_name: &str) -> Result<&TenantAttribute> {
        if self.tenants.contains_key(package_name) {
            Ok(self.tenants.get(package_name).unwrap())
        } else {
            bail!("Tenant not found: {:?}", package_name);
        }
    }

    /// Returns an iterator over tenants.
    pub fn list_tenants_info(&self) -> impl Iterator<Item = (&String, &TenantAttribute)> {
        self.tenants.iter()
    }
}

// TODO(rkrohit): Update the TenancySpec to keep the fields that are required for replay protection
// for tenants before releasing the multi-tenancy feature.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TenancySpec {
    pub tenants: HashMap<String, (TenantPackageInfo, TenantAttribute)>,
}

impl TenancySpec {
    // Validation logic for tenants against the spec. This will typically be needed when validation
    // of tenants is required against the spec from the previous boot to prevent against attacks
    // such as rollback. Precisely, the method checks the following:
    // 1. For any tenant which also exists in the spec, its certificate hash must not have changed.
    // 2. For any such tenant, its version (rollback_index or version_code) must not be lower than
    //    the version in the spec.
    // 3. Tenants not existing in the spec are permitted. (There can new tenants not present in
    //    previous boot. They are assumed to have been validated against the `TenantConfig`).
    // 4. There can be tenants in spec absent in the input. (Tenants are allowed to be removed.)
    pub(crate) fn validate_tenant_apks_against_instance_spec(
        &self,
        tenant_apks: &[ApkData],
    ) -> Result<()> {
        let spec_apk_tenants: HashMap<_, _> = self
            .tenants
            .iter()
            .filter_map(|(name, (info, _))| {
                if let TenantPackageInfo::ApkData(apk_data) = info {
                    Some((name.as_str(), apk_data))
                } else {
                    None
                }
            })
            .collect();
        let provided_tenant_names: HashSet<_> =
            tenant_apks.iter().map(|t| t.package_name.as_str()).collect();

        let removed_tenants: Vec<_> = spec_apk_tenants
            .keys()
            .filter(|name| !provided_tenant_names.contains(*name))
            .copied()
            .collect();
        if !removed_tenants.is_empty() {
            warn!("Removed tenant APKs: {:?}", removed_tenants);
        }

        let mut added_tenants = Vec::new();
        for provided_tenant in tenant_apks {
            if let Some(spec_apk_data) = spec_apk_tenants.get(provided_tenant.package_name.as_str())
            {
                // 1. Certificate hash must not change
                if provided_tenant.cert_hash != spec_apk_data.cert_hash {
                    bail!(MicrodroidError::PayloadVerificationFailed(format!(
                        "Certificate hash for tenant {} changed. Spec: {:?}, Provided: {:?}",
                        provided_tenant.package_name,
                        spec_apk_data.cert_hash,
                        provided_tenant.cert_hash
                    )));
                }

                // 2. Version must not be lower
                if provided_tenant.version_code < spec_apk_data.version_code {
                    bail!(MicrodroidError::PayloadVerificationFailed(format!(
                        "Version rollback for tenant {}. Spec: {:?}, Provided: {:?}",
                        provided_tenant.package_name,
                        spec_apk_data.version_code,
                        provided_tenant.version_code
                    )));
                }
            } else {
                added_tenants.push(provided_tenant.package_name.as_str());
            }
        }
        if !added_tenants.is_empty() {
            info!("Added tenant APKs: {:?}", added_tenants);
        }
        Ok(())
    }
}

pub(crate) fn validate_tenants_against_existing_spec_update_spec(
    is_new_instance: bool,
    instance_disk: &mut InstanceDisk,
    tenant_manager: &TenantManager,
    tenant_apks_data: Vec<ApkData>,
    tenant_apex_data: Vec<ApexData>,
) -> Result<()> {
    // On all but first run of the VM, there must be a pre-exiting InstanceSpec which the tenants
    // must be validated against.
    // This needs to be done before `current_instance_spec` is constructed, so we can borrow
    // `tenant_apks_data` before it\'s moved.
    let loaded_instance_spec = if is_new_instance {
        None
    } else {
        info!("Subsequent boot, loading and validating InstanceSpec");
        let spec = instance_disk
            .read_instance_spec()
            .with_context(|| "Failed to read instance spec on subsequent boot")?
            .ok_or_else(|| anyhow!("InstanceSpec not found on subsequent boot"))?;

        spec.tenancy_spec
            .validate_tenant_apks_against_instance_spec(&tenant_apks_data)
            .with_context(|| "Validation of tenants against loaded instance spec failed")?;
        Some(spec)
    };

    // Construct an InstanceSpec from the CURRENTLY presented tenants
    let current_instance_spec = {
        let mut tenants = HashMap::new();
        for apk_data in tenant_apks_data {
            let attribute = tenant_manager.get_tenant_attribute(&apk_data.package_name)?;
            tenants.insert(
                apk_data.package_name.clone(),
                (TenantPackageInfo::ApkData(apk_data), *attribute),
            );
        }
        for apex_data in tenant_apex_data {
            let attribute = tenant_manager.get_tenant_attribute(&apex_data.name)?;
            tenants.insert(
                apex_data.name.clone(),
                (TenantPackageInfo::ApexData(apex_data), *attribute),
            );
        }
        InstanceSpec { tenancy_spec: TenancySpec { tenants } }
    };

    if let Some(loaded_instance_spec) = loaded_instance_spec {
        // Subsequent boot: update spec on disk if it has changed.
        if loaded_instance_spec != current_instance_spec {
            info!("InstanceSpec changed, updating on disk");
            instance_disk
                .write_instance_spec(&current_instance_spec)
                .context("Failed to update instance spec on disk")?;
        }
    } else {
        // New instance: write the newly created spec.
        info!("New instance, creating and writing InstanceSpec");
        instance_disk
            .write_instance_spec(&current_instance_spec)
            .with_context(|| "Failed to write instance spec for new instance")?;
    }
    Ok(())
}
