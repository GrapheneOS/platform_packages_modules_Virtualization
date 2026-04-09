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

//! Tenant Configuration validation logic.

use crate::instance::{ApexData, ApkData};
use crate::MicrodroidError;
use anyhow::{anyhow, bail, Result};
use bssl_crypto::digest::Sha512;
use microdroid_payload_config::TenantConfig;
use std::collections::{HashMap, HashSet};

// Validation logic includes:
// 1. The tenant_apk and tenant_apex exactly matches apks and apexes described in tenant_config
//    (comparison is by package name)
// 2. The order of description of tenants in tenant_config is irrelevant.
// 3. The rollback_index (or version_code if rollback_index is missing) >=  min_version in
//    tenant_config
// 4. The cert_hash of tenant apk (or sha512  of public key of tenant apex) == expected_authority in
//    tenant_config
pub(crate) fn validate_tenants_against_tenant_config(
    tenant_apk: &[ApkData],
    tenant_apex: &[ApexData],
    tenant_config: &[TenantConfig],
) -> Result<()> {
    let apex_map: HashMap<&str, &ApexData> =
        tenant_apex.iter().map(|apex| (apex.name.as_str(), apex)).collect();
    let apk_map: HashMap<&str, &ApkData> =
        tenant_apk.iter().map(|apk| (apk.package_name.as_str(), apk)).collect();

    let apex_configs_count = tenant_config
        .iter()
        .filter_map(|c| match c {
            TenantConfig::Apex(config) => Some(&config.name),
            _ => None,
        })
        .collect::<HashSet<_>>()
        .len();
    if apex_map.len() != apex_configs_count {
        bail!(MicrodroidError::PayloadInvalidConfig(
            "Provided tenant APEXes do not match the configuration".to_string()
        ));
    }

    let apk_configs_count = tenant_config
        .iter()
        .filter_map(|c| match c {
            TenantConfig::Apk(config) => Some(&config.name),
            _ => None,
        })
        .collect::<HashSet<_>>()
        .len();
    if apk_map.len() != apk_configs_count {
        bail!(MicrodroidError::PayloadInvalidConfig(
            "Provided tenant APKs do not match the configuration".to_string()
        ));
    }
    for tenant_config_item in tenant_config {
        let (config, type_name, version_res, authority_hash, auth_name) = match tenant_config_item {
            TenantConfig::Apex(config) => {
                let apex_data = apex_map.get(config.name.as_str()).ok_or_else(|| {
                    MicrodroidError::PayloadInvalidConfig(format!(
                        "APEX tenant '{}' from config not provided",
                        config.name
                    ))
                })?;
                let version_res = apex_data.manifest_version.map(|v| v as u64).ok_or_else(|| {
                    anyhow!(
                        "APEX ('{}') is missing manifest_version, but min_version is specified",
                        &config.name
                    )
                });
                let authority_hash = hex::encode(Sha512::hash(&apex_data.public_key));
                (config, "APEX", version_res, authority_hash, "authority_hash")
            }
            TenantConfig::Apk(config) => {
                let apk_data = apk_map.get(config.name.as_str()).ok_or_else(|| {
                    MicrodroidError::PayloadInvalidConfig(format!(
                        "APK tenant '{}' from config not provided",
                        config.name
                    ))
                })?;
                let version_res =
                    Ok(apk_data.rollback_index.map_or(apk_data.version_code, u64::from));
                let authority_hash = hex::encode(&apk_data.cert_hash);
                (config, "APK", version_res, authority_hash, "cert_hash")
            }
        };

        let version = version_res?;
        if version < config.min_version {
            bail!(MicrodroidError::PayloadInvalidConfig(format!(
                "{} ('{}') version ({}) is less than min_version ({})",
                type_name, &config.name, version, config.min_version
            )));
        }

        let expected_auth = config.expected_authority.resolve_authority();
        if !expected_auth.is_empty() && expected_auth != authority_hash {
            bail!(MicrodroidError::PayloadInvalidConfig(format!(
                "{} ('{}') {} ('{}') mismatches expected authority ({})",
                type_name, &config.name, auth_name, authority_hash, expected_auth
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::EncryptedStoreMode;
    use microdroid_payload_config::{ExpectedAuthority, TenantConfiguration};

    fn create_tenant_config_apk(
        package_name: &str,
        version_code: u64,
        rollback_index: Option<u32>,
        cert_hash: &[u8],
    ) -> ApkData {
        ApkData {
            root_hash: vec![],
            cert_hash: cert_hash.to_vec(),
            package_name: package_name.to_string(),
            version_code,
            rollback_index,
            encrypted_store_mode: EncryptedStoreMode::DefaultKey,
        }
    }

    fn create_apex_data(name: &str, manifest_version: Option<i64>, public_key: &[u8]) -> ApexData {
        ApexData {
            name: name.to_string(),
            manifest_name: Some(name.to_string()),
            manifest_version,
            public_key: public_key.to_vec(),
            root_digest: vec![],
            last_update_seconds: 0,
            is_factory: false,
        }
    }

    fn create_authority(expected_authority: Option<String>) -> ExpectedAuthority {
        let auth = expected_authority.unwrap_or_default();
        ExpectedAuthority { dev_key: auth.clone(), test_key: auth.clone(), release_key: auth }
    }

    fn create_apk_config(
        name: &str,
        min_version: u64,
        expected_authority: Option<String>,
    ) -> TenantConfig {
        TenantConfig::Apk(TenantConfiguration {
            name: name.to_string(),
            uid: 10000,
            task: None,
            min_version,
            expected_authority: create_authority(expected_authority),
            cgroup_config: None,
        })
    }

    fn create_tenant_config_apex(
        name: &str,
        min_version: u64,
        expected_authority: Option<String>,
    ) -> TenantConfig {
        TenantConfig::Apex(TenantConfiguration {
            name: name.to_string(),
            uid: 10000,
            task: None,
            min_version,
            expected_authority: create_authority(expected_authority),
            cgroup_config: None,
        })
    }

    #[test]
    fn test_valid_tenants() {
        let apk_cert_hash = [1; 32];
        let apex_public_key = [2; 32];
        let tenant_apk = vec![create_tenant_config_apk("com.test.apk", 3, Some(5), &apk_cert_hash)];
        let tenant_apex = vec![create_apex_data("com.test.apex", Some(20), &apex_public_key)];
        let tenant_config = vec![
            create_apk_config("com.test.apk", 5, Some(hex::encode(apk_cert_hash))),
            create_tenant_config_apex(
                "com.test.apex",
                20,
                Some(hex::encode(Sha512::hash(&apex_public_key))),
            ),
        ];

        assert!(validate_tenants_against_tenant_config(&tenant_apk, &tenant_apex, &tenant_config)
            .is_ok());
    }

    #[test]
    fn test_mismatched_apk_count() {
        let tenant_apk = vec![];
        let tenant_apex = vec![];
        let tenant_config = vec![create_apk_config("com.test.apk", 0, None)];

        let result =
            validate_tenants_against_tenant_config(&tenant_apk, &tenant_apex, &tenant_config);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            MicrodroidError::PayloadInvalidConfig(
                "Provided tenant APKs do not match the configuration".to_string()
            )
            .to_string()
        );
    }

    #[test]
    fn test_mismatched_apex_count() {
        let tenant_apk = vec![];
        let tenant_apex = vec![];
        let tenant_config = vec![create_tenant_config_apex("com.test.apex", 0, None)];

        let result =
            validate_tenants_against_tenant_config(&tenant_apk, &tenant_apex, &tenant_config);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            MicrodroidError::PayloadInvalidConfig(
                "Provided tenant APEXes do not match the configuration".to_string()
            )
            .to_string()
        );
    }

    #[test]
    fn test_missing_apk_tenant() {
        let tenant_apk = vec![create_tenant_config_apk("com.test.apk1", 1, None, &[1])];
        let tenant_config = vec![create_apk_config("com.test.apk2", 0, None)];
        let result = validate_tenants_against_tenant_config(&tenant_apk, &[], &tenant_config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("APK tenant 'com.test.apk2' from config not provided"));
    }

    #[test]
    fn test_apk_version_rollback() {
        let apk_cert_hash = [1; 32];
        let tenant_apk = vec![create_tenant_config_apk("com.test.apk", 9, None, &apk_cert_hash)];
        let tenant_apex = vec![];
        let tenant_config =
            vec![create_apk_config("com.test.apk", 10, Some(hex::encode(apk_cert_hash)))];

        let result =
            validate_tenants_against_tenant_config(&tenant_apk, &tenant_apex, &tenant_config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("version (9) is less than min_version (10)"));
    }

    #[test]
    fn test_apk_rollback_index_version_rollback() {
        let apk_cert_hash = [1; 32];
        let tenant_apk =
            vec![create_tenant_config_apk("com.test.apk", 10, Some(4), &apk_cert_hash)];
        let tenant_apex = vec![];
        let tenant_config =
            vec![create_apk_config("com.test.apk", 5, Some(hex::encode(apk_cert_hash)))];

        let result =
            validate_tenants_against_tenant_config(&tenant_apk, &tenant_apex, &tenant_config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("version (4) is less than min_version (5)"));
    }

    #[test]
    fn test_apex_version_rollback() {
        let apex_public_key = [2; 32];
        let tenant_apk = vec![];
        let tenant_apex = vec![create_apex_data("com.test.apex", Some(19), &apex_public_key)];
        let tenant_config = vec![create_tenant_config_apex(
            "com.test.apex",
            20,
            Some(hex::encode(Sha512::hash(&apex_public_key))),
        )];

        let result =
            validate_tenants_against_tenant_config(&tenant_apk, &tenant_apex, &tenant_config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("version (19) is less than min_version (20)"));
    }

    #[test]
    fn test_apk_authority_mismatch() {
        let apk_cert_hash = [1; 32];
        let wrong_hash = [3; 32];
        let tenant_apk = vec![create_tenant_config_apk("com.test.apk", 10, None, &apk_cert_hash)];
        let tenant_apex = vec![];
        let tenant_config =
            vec![create_apk_config("com.test.apk", 10, Some(hex::encode(wrong_hash)))];

        let result =
            validate_tenants_against_tenant_config(&tenant_apk, &tenant_apex, &tenant_config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("mismatches expected authority"));
    }

    #[test]
    fn test_apex_authority_mismatch() {
        let apex_public_key = [2; 32];
        let wrong_key = [4; 32];
        let tenant_apk = vec![];
        let tenant_apex = vec![create_apex_data("com.test.apex", Some(20), &apex_public_key)];
        let tenant_config = vec![create_tenant_config_apex(
            "com.test.apex",
            20,
            Some(hex::encode(Sha512::hash(&wrong_key))),
        )];

        let result =
            validate_tenants_against_tenant_config(&tenant_apk, &tenant_apex, &tenant_config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("mismatches expected authority"));
    }

    #[test]
    fn test_empty_expected_authority() {
        let apk_cert_hash = [1; 32];
        let tenant_apk = vec![create_tenant_config_apk("com.test.apk", 10, None, &apk_cert_hash)];
        let tenant_config = vec![create_apk_config("com.test.apk", 10, Some("".to_string()))];

        assert!(validate_tenants_against_tenant_config(&tenant_apk, &[], &tenant_config).is_ok());
    }

    #[test]
    fn test_missing_min_version_fails_deserialization() {
        let json_str = r#"{
            "package": "apk",
            "name": "com.test.apk",
            "uid": 10000,
            "expected_authority": {
                "dev-keys": "some_authority",
                "test-keys": "some_authority",
                "release-keys": "some_authority"
            }
        }"#;

        let result: Result<TenantConfig, _> = serde_json::from_str(json_str);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing field `min_version`"));
    }

    #[test]
    fn test_apex_missing_manifest_version() {
        let apex_public_key = [2; 32];
        let tenant_apex = vec![create_apex_data("com.test.apex", None, &apex_public_key)];
        let tenant_config = vec![create_tenant_config_apex(
            "com.test.apex",
            20,
            Some(hex::encode(Sha512::hash(&apex_public_key))),
        )];

        let result = validate_tenants_against_tenant_config(&[], &tenant_apex, &tenant_config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("is missing manifest_version, but min_version is specified"));
    }

    #[test]
    fn test_apk_with_map_authority() {
        let apk_cert_hash = [1; 32];
        let tenant_apk = [create_tenant_config_apk("com.test.apk", 10, None, &apk_cert_hash)];
        // To avoid flakiness depending on the host's build type (userdebug vs release),
        // we set all keys to the matching hash. This ensures validate_tenants_against_tenant_config
        // succeeds regardless of which key resolve_authority() selects.
        let correct_hash = hex::encode(apk_cert_hash);
        let authority = ExpectedAuthority {
            dev_key: correct_hash.clone(),
            test_key: correct_hash.clone(),
            release_key: correct_hash,
        };
        let tenant_config = [TenantConfig::Apk(TenantConfiguration {
            name: "com.test.apk".to_string(),
            uid: 10000,
            task: None,
            min_version: 10,
            expected_authority: authority,
            cgroup_config: None,
        })];

        assert!(validate_tenants_against_tenant_config(&tenant_apk, &[], &tenant_config).is_ok());
    }
}
