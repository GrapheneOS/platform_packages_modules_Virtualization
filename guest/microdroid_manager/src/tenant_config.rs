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
use microdroid_payload_config::TenantConfig;
use openssl::sha::sha512;
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
                let version_res = if config.min_version.is_some() {
                    apex_data.manifest_version.map(|v| v as u64).ok_or_else(|| {
                        anyhow!(
                            "APEX ('{}') is missing manifest_version, but min_version is specified",
                            &config.name
                        )
                    })
                } else {
                    Ok(apex_data.manifest_version.map(|v| v as u64).unwrap_or(0))
                };
                let authority_hash = hex::encode(sha512(&apex_data.public_key));
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

        if let Some(min_version) = config.min_version {
            let version = version_res?;
            if version < min_version {
                bail!(MicrodroidError::PayloadInvalidConfig(format!(
                    "{} ('{}') version ({}) is less than min_version ({})",
                    type_name, &config.name, version, min_version
                )));
            }
        }
        if let Some(expected_auth) = &config.expected_authority {
            if !expected_auth.is_empty() && *expected_auth != authority_hash {
                bail!(MicrodroidError::PayloadInvalidConfig(format!(
                    "{} ('{}') {} ('{}') mismatches expected authority ({})",
                    type_name, &config.name, auth_name, authority_hash, expected_auth
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::EncryptedStoreMode;
    use microdroid_payload_config::TenantConfiguration;

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

    fn create_apk_config(
        name: &str,
        min_version: Option<u64>,
        expected_authority: Option<String>,
    ) -> TenantConfig {
        TenantConfig::Apk(TenantConfiguration {
            name: name.to_string(),
            task: None,
            min_version,
            expected_authority,
        })
    }

    fn create_tenant_config_apex(
        name: &str,
        min_version: Option<u64>,
        expected_authority: Option<String>,
    ) -> TenantConfig {
        TenantConfig::Apex(TenantConfiguration {
            name: name.to_string(),
            task: None,
            min_version,
            expected_authority,
        })
    }

    #[test]
    fn test_valid_tenants() {
        let apk_cert_hash = [1; 32];
        let apex_public_key = [2; 32];
        let tenant_apk = vec![create_tenant_config_apk("com.test.apk", 3, Some(5), &apk_cert_hash)];
        let tenant_apex = vec![create_apex_data("com.test.apex", Some(20), &apex_public_key)];
        let tenant_config = vec![
            create_apk_config("com.test.apk", Some(5), Some(hex::encode(apk_cert_hash))),
            create_tenant_config_apex(
                "com.test.apex",
                Some(20),
                Some(hex::encode(sha512(&apex_public_key))),
            ),
        ];

        assert!(validate_tenants_against_tenant_config(&tenant_apk, &tenant_apex, &tenant_config)
            .is_ok());
    }

    #[test]
    fn test_mismatched_apk_count() {
        let tenant_apk = vec![];
        let tenant_apex = vec![];
        let tenant_config = vec![create_apk_config("com.test.apk", None, None)];

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
        let tenant_config = vec![create_tenant_config_apex("com.test.apex", None, None)];

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
        let tenant_config = vec![create_apk_config("com.test.apk2", None, None)];
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
            vec![create_apk_config("com.test.apk", Some(10), Some(hex::encode(apk_cert_hash)))];

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
            vec![create_apk_config("com.test.apk", Some(5), Some(hex::encode(apk_cert_hash)))];

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
            Some(20),
            Some(hex::encode(sha512(&apex_public_key))),
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
            vec![create_apk_config("com.test.apk", Some(10), Some(hex::encode(wrong_hash)))];

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
            Some(20),
            Some(hex::encode(sha512(&wrong_key))),
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
        let tenant_config = vec![create_apk_config("com.test.apk", Some(10), Some("".to_string()))];

        assert!(validate_tenants_against_tenant_config(&tenant_apk, &[], &tenant_config).is_ok());
    }

    #[test]
    fn test_no_min_version() {
        let apk_cert_hash = [1; 32];
        let tenant_apk = vec![create_tenant_config_apk("com.test.apk", 10, None, &apk_cert_hash)];
        let tenant_config =
            vec![create_apk_config("com.test.apk", None, Some(hex::encode(apk_cert_hash)))];

        assert!(validate_tenants_against_tenant_config(&tenant_apk, &[], &tenant_config).is_ok());
    }

    #[test]
    fn test_apex_missing_manifest_version() {
        let apex_public_key = [2; 32];
        let tenant_apex = vec![create_apex_data("com.test.apex", None, &apex_public_key)];
        let tenant_config = vec![create_tenant_config_apex(
            "com.test.apex",
            Some(20),
            Some(hex::encode(sha512(&apex_public_key))),
        )];

        let result = validate_tenants_against_tenant_config(&[], &tenant_apex, &tenant_config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("is missing manifest_version, but min_version is specified"));
    }
}
