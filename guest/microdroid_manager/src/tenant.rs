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

use super::instance::{ApexData, ApkData};
use anyhow::{bail, Result};
use log::{info, warn};
use microdroid_payload_config::TenantConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TenantPackageInfo {
    ApkData(ApkData),
    ApexData(ApexData),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
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
