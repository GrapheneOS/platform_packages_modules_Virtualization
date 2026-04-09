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
use crate::Task;
use anyhow::{anyhow, bail, Context, Result};
use log::{info, warn};
use microdroid_payload_config::TenantConfig;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TenantPackageInfo {
    ApkData(ApkData),
    ApexData(ApexData),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub(crate) struct TenantAttribute {
    uid: u32,
    // Domains requiring setexeccon-based domain transitions.
    // If None, auto-transition into `microdroid_app` happens.
    selinux_domain: Option<CString>,
}

impl TenantAttribute {
    pub fn uid(&self) -> u32 {
        self.uid
    }

    pub fn gid() -> u32 {
        microdroid_uids::MICRODROID_PAYLOAD_GID
    }

    pub fn selinux_domain(&self) -> Option<CString> {
        self.selinux_domain.clone()
    }
}

#[derive(Debug)]
pub(crate) struct TenantManager {
    tenants: HashMap<String, TenantAttribute>,
}

impl TenantManager {
    fn new() -> Self {
        Self {
            // TODO(basantwani): Add persistence by integrating with InstanceSpec
            tenants: HashMap::new(),
        }
    }

    pub fn initialize(tenants_config: &[TenantConfig]) -> Result<Self> {
        let mut manager = Self::new();
        for tenant in tenants_config {
            let (tenant, uid) = match tenant {
                TenantConfig::Apex(c) => (c, c.uid),
                TenantConfig::Apk(c) => (c, c.uid),
            };
            manager.register_tenant_package(&tenant.name, uid, tenant.task.as_ref())?;
        }
        Ok(manager)
    }

    fn register_tenant_package(
        &mut self,
        package_name: &str,
        uid: u32,
        task: Option<&Task>,
    ) -> Result<()> {
        if self.tenants.contains_key(package_name) {
            bail!(MicrodroidError::PayloadInvalidConfig(format!(
                "Duplicate tenant name found during registration: {:?}",
                package_name,
            )));
        }

        if !(microdroid_uids::MICRODROID_TENANT_UID_RANGE_START
            ..=microdroid_uids::MICRODROID_TENANT_UID_RANGE_END)
            .contains(&uid)
        {
            bail!(MicrodroidError::PayloadInvalidConfig(format!(
                "Tenant UID {} is invalid. It must be in range [{}, {}]",
                uid,
                microdroid_uids::MICRODROID_TENANT_UID_RANGE_START,
                microdroid_uids::MICRODROID_TENANT_UID_RANGE_END
            )));
        }

        if self.tenants.values().any(|t| t.uid == uid) {
            bail!(MicrodroidError::PayloadInvalidConfig(format!(
                "Duplicate tenant UID found: {}",
                uid
            )));
        }

        let selinux_domain = if let Some(Task { selinux_type: Some(selinux_type), .. }) = task {
            let selinux_type_str = selinux_type.to_str().expect("SELinux type must be valid UTF-8");
            Some(CString::new(format!("u:r:{selinux_type_str}:s0")).unwrap())
        } else {
            None
        };

        let attribute = TenantAttribute { uid, selinux_domain };
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

    pub fn get_tenant_package_name(&self, uid: i64) -> Result<String> {
        let uid = uid as u32;
        self.list_tenants_info()
            .find(|&(_, attribute)| attribute.uid() == uid)
            .map(|(name, _)| name.clone())
            .ok_or_else(|| anyhow!("Tenant not found for uid: {:?}", uid))
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

// TODO(b/441899073) Microdroid manager does not yet persist or validate the tenants data against
// instance spec.
#[allow(dead_code)]
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
                (TenantPackageInfo::ApkData(apk_data), attribute.clone()),
            );
        }
        for apex_data in tenant_apex_data {
            let attribute = tenant_manager.get_tenant_attribute(&apex_data.name)?;
            tenants.insert(
                apex_data.name.clone(),
                (TenantPackageInfo::ApexData(apex_data), attribute.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use microdroid_payload_config::{ExpectedAuthority, TenantConfiguration};

    fn create_tenant_config(name: &str, uid: u32) -> TenantConfig {
        TenantConfig::Apk(TenantConfiguration {
            name: name.to_string(),
            uid,
            task: None,
            min_version: 1,
            expected_authority: ExpectedAuthority {
                dev_key: "".to_string(),
                test_key: "".to_string(),
                release_key: "".to_string(),
            },
            cgroup_config: None,
        })
    }

    #[test]
    fn test_initialize_valid_tenants() {
        let configs = vec![
            create_tenant_config("com.example.tenant1", 10000),
            create_tenant_config("com.example.tenant2", 10001),
        ];
        let manager = TenantManager::initialize(&configs);
        assert!(manager.is_ok());
        let manager = manager.unwrap();
        let tenant1_attr = manager.get_tenant_attribute("com.example.tenant1").unwrap();
        assert_eq!(tenant1_attr.uid, 10000);
        let tenant2_attr = manager.get_tenant_attribute("com.example.tenant2").unwrap();
        assert_eq!(tenant2_attr.uid, 10001);
    }

    #[test]
    fn test_initialize_duplicate_name() {
        let configs = vec![
            create_tenant_config("com.example.tenant1", 10000),
            create_tenant_config("com.example.tenant1", 10001),
        ];
        let err = TenantManager::initialize(&configs).unwrap_err();
        assert!(err.to_string().contains("Duplicate tenant name"));
    }

    #[test]
    fn test_initialize_duplicate_uid() {
        let configs = vec![
            create_tenant_config("com.example.tenant1", 10000),
            create_tenant_config("com.example.tenant2", 10000),
        ];
        let err = TenantManager::initialize(&configs).unwrap_err();
        assert!(err.to_string().contains("Duplicate tenant UID"));
    }

    #[test]
    fn test_initialize_uid_outside_range() {
        let err_low =
            TenantManager::initialize(&[create_tenant_config("com.example.tenant1", 9999)])
                .unwrap_err();
        assert!(err_low.to_string().contains("Tenant UID 9999 is invalid"));

        let err_high =
            TenantManager::initialize(&[create_tenant_config("com.example.tenant1", 65535)])
                .unwrap_err();
        assert!(err_high.to_string().contains("Tenant UID 65535 is invalid"));
    }
}
