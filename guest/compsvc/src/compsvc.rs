/*
 * Copyright (C) 2021 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! compsvc is a service to run compilation tasks in a PVM upon request. It is able to set up
//! file descriptors backed by authfs (via authfs_service) and pass the file descriptors to the
//! actual compiler.

use crate::artifact_signer::ArtifactSigner;
use crate::compilation::{odrefresh, run_dex2oat};
#[cfg(test)]
use crate::compos_key::mock_wrapper as compos_key;
#[cfg(not(test))]
use crate::compos_key::wrapper as compos_key;
use anyhow::{bail, Context, Result};
use log::{error, info};
use std::default::Default;
use std::fs::read_dir;
use std::iter::zip;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[cfg(not(test))]
use {crate::wrappers::AuthFsFactory, compos_wrappers::system_properties};

#[cfg(test)]
use {
    crate::wrappers::MockAuthFsFactory as AuthFsFactory,
    compos_wrappers_with_mocks::mock_system_properties as system_properties,
};

use authfs_aidl_interface::aidl::com::android::virt::fs::IAuthFsService::IAuthFsService;
use binder::{
    BinderFeatures, ExceptionCode, Interface, IntoBinderResult, Result as BinderResult, Strong,
};
use compos_aidl_interface::aidl::com::android::compos::{
    ICompOsService::{BnCompOsService, ICompOsService, OdrefreshArgs::OdrefreshArgs},
    IVerifiedDex2OatService::{
        BnVerifiedDex2OatService, Dex2OatArg::Dex2OatArg, IVerifiedDex2OatService,
    },
    IVerifiedDex2OatTaskCallback::IVerifiedDex2OatTaskCallback,
};
use compos_common::binder::to_binder_result;
use compos_common::odrefresh::{is_system_property_interesting, ODREFRESH_PATH};

trait ProvidesVmManagement {
    fn set_initialized(&self, initialized: bool);

    fn get_initialized(&self) -> Option<bool>;

    fn do_initialize_system_properties(
        &self,
        names: &[String],
        values: &[String],
    ) -> BinderResult<()> {
        let initialized = self.get_initialized();
        if initialized.is_some() {
            return Err(format!("Already initialized: {initialized:?}"))
                .or_binder_exception(ExceptionCode::ILLEGAL_STATE);
        }
        self.set_initialized(false);
        if names.len() != values.len() {
            return Err(format!(
                "Received inconsistent number of keys ({}) and values ({})",
                names.len(),
                values.len()
            ))
            .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
        }
        for (name, value) in zip(names, values) {
            if !is_system_property_interesting(name) {
                return Err(format!("Received invalid system property {name}"))
                    .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
            }
            let result = system_properties::write(name, value);
            if result.is_err() {
                error!("Failed to setprop {}", &name);
                return to_binder_result(result);
            }
        }
        self.set_initialized(true);
        Ok(())
    }

    fn do_check_initialized(&self) -> BinderResult<()> {
        if !self.get_initialized().unwrap_or(false) {
            return Err("Service has not been initialized")
                .or_binder_exception(ExceptionCode::ILLEGAL_STATE);
        }
        Ok(())
    }

    fn do_get_public_key(&self) -> BinderResult<Vec<u8>> {
        to_binder_result(compos_key::get_public_key())
    }

    fn do_get_attestation_chain(&self) -> BinderResult<Vec<u8>> {
        to_binder_result(compos_key::get_attestation_chain())
    }

    fn do_quit(&self) -> BinderResult<()> {
        // When our process exits, Microdroid will shut down the VM.
        info!("Received quit request, exiting");
        std::process::exit(0);
    }
}

/// Constructs a binder object that implements ICompOsService.
pub fn new_odrefresh_binder() -> Result<Strong<dyn ICompOsService>> {
    let service = CompOsService {
        odrefresh_path: PathBuf::from(ODREFRESH_PATH),
        initialized: RwLock::new(None),
    };
    Ok(BnCompOsService::new_binder(service, BinderFeatures::default()))
}

struct CompOsService {
    odrefresh_path: PathBuf,

    /// A locked protected tri-state.
    ///  * None: uninitialized
    ///  * Some(true): initialized successfully
    ///  * Some(false): failed to initialize
    initialized: RwLock<Option<bool>>,
}

impl Interface for CompOsService {}

impl ProvidesVmManagement for CompOsService {
    fn get_initialized(&self) -> Option<bool> {
        *self.initialized.read().unwrap()
    }
    fn set_initialized(&self, initialized: bool) {
        *self.initialized.write().unwrap() = Some(initialized);
    }
}

impl ICompOsService for CompOsService {
    fn initializeSystemProperties(&self, names: &[String], values: &[String]) -> BinderResult<()> {
        self.do_initialize_system_properties(names, values)
    }

    fn odrefresh(&self, args: &OdrefreshArgs) -> BinderResult<i8> {
        self.do_check_initialized()?;
        to_binder_result(self.do_odrefresh(args))
    }

    fn getPublicKey(&self) -> BinderResult<Vec<u8>> {
        self.do_get_public_key()
    }

    fn getAttestationChain(&self) -> BinderResult<Vec<u8>> {
        self.do_get_attestation_chain()
    }

    fn quit(&self) -> BinderResult<()> {
        self.do_quit()
    }
}

impl CompOsService {
    fn do_odrefresh(&self, args: &OdrefreshArgs) -> Result<i8> {
        let authfs_service: Strong<dyn IAuthFsService> = AuthFsFactory::new_authfs_service()?;
        let exit_code = odrefresh(&self.odrefresh_path, args, authfs_service, |output_dir| {
            // authfs only shows us the files we created, so it's ok to just sign everything
            // under the output directory.
            let mut artifact_signer = ArtifactSigner::new(&output_dir);
            add_artifacts(&output_dir, &mut artifact_signer)?;

            artifact_signer.write_info_and_signature(&output_dir.join("compos.info"))
        })
        .context("odrefresh failed")?;
        Ok(exit_code as i8)
    }
}

fn add_artifacts(target_dir: &Path, artifact_signer: &mut ArtifactSigner) -> Result<()> {
    for entry in
        read_dir(target_dir).with_context(|| format!("Traversing {}", target_dir.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            add_artifacts(&entry.path(), artifact_signer)?;
        } else if file_type.is_file() {
            artifact_signer.add_artifact(&entry.path())?;
        } else {
            // authfs shouldn't create anything else, but just in case
            bail!("Unexpected file type in artifacts: {:?}", entry);
        }
    }
    Ok(())
}

/// Constructs a binderobject that implements IVerifiedDex2OatService
pub fn new_dex2oat_binder() -> Result<Strong<dyn IVerifiedDex2OatService>> {
    let service = VerifiedDex2OatService { initialized: RwLock::new(None) };
    Ok(BnVerifiedDex2OatService::new_binder(service, BinderFeatures::default()))
}

struct VerifiedDex2OatService {
    /// A locked protected tri-state.
    ///  * None: uninitialized
    ///  * Some(true): initialized successfully
    ///  * Some(false): failed to initialize
    initialized: RwLock<Option<bool>>,
}

impl Interface for VerifiedDex2OatService {}

impl ProvidesVmManagement for VerifiedDex2OatService {
    fn get_initialized(&self) -> Option<bool> {
        *self.initialized.read().unwrap()
    }
    fn set_initialized(&self, initialized: bool) {
        *self.initialized.write().unwrap() = Some(initialized);
    }
}

impl IVerifiedDex2OatService for VerifiedDex2OatService {
    fn initializeSystemProperties(&self, names: &[String], values: &[String]) -> BinderResult<()> {
        self.do_initialize_system_properties(names, values)
    }

    fn verifiedDex2Oat(
        &self,
        args: &[Dex2OatArg],
        system_dir_fd: i32,
        system_ext_dir_fd: i32,
        manifest_fd: i32,
        cb: &Strong<dyn IVerifiedDex2OatTaskCallback>,
    ) -> BinderResult<()> {
        self.do_check_initialized()?;
        to_binder_result(run_dex2oat(args, system_dir_fd, system_ext_dir_fd, manifest_fd, cb))
    }

    fn getPublicKey(&self) -> BinderResult<Vec<u8>> {
        self.do_get_public_key()
    }

    fn quit(&self) -> BinderResult<()> {
        self.do_quit()
    }
}

#[cfg(test)]
mod test {
    use crate::compsvc::CompOsService;
    use crate::wrappers::{mock_command_line_helper, MockAuthFsFactory};
    use authfs_aidl_interface::aidl::com::android::virt::fs::{
        AuthFsConfig::AuthFsConfig, IAuthFs::MockIAuthFs, IAuthFsService::MockIAuthFsService,
    };
    use binder::{ExceptionCode, Strong};
    use compos_aidl_interface::aidl::com::android::compos::ICompOsService::{
        CompilationMode::CompilationMode, ICompOsService, OdrefreshArgs::OdrefreshArgs,
    };
    use compos_common::odrefresh::ODREFRESH_PATH;
    use compos_wrappers_with_mocks::{
        minijail::{
            Command as mock_minijail_command, MockCommandFactory as mock_minijail_command_factory,
            MockMinijail,
        },
        mock_system_properties,
    };
    use mockall::predicate::eq;
    use std::{
        collections::HashSet,
        os::fd::RawFd,
        path::{Path, PathBuf},
        sync::{Mutex, RwLock},
    };
    use tempfile::tempdir;

    // Setting expectations on mocks of static methods requires the use of a mutex for
    // serialization.
    static MTX: Mutex<()> = Mutex::new(());

    #[test]
    fn odrefresh_fails_on_not_initialized() {
        let _m = MTX.lock();
        let args: OdrefreshArgs = Default::default();
        let odrefresh_svc = CompOsService {
            odrefresh_path: PathBuf::from(ODREFRESH_PATH),
            initialized: RwLock::new(None),
        };
        let result = odrefresh_svc.odrefresh(&args);
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().exception_code(), ExceptionCode::ILLEGAL_STATE);
    }

    #[test]
    fn odrefresh_succeeds() {
        let _m = MTX.lock();
        let args = OdrefreshArgs {
            systemDirFd: 1,
            outputDirFd: 2,
            stagingDirFd: 3,
            zygoteArch: "zygote64".to_string(),
            targetDirName: "target".to_string(),
            systemServerCompilerFilter: "filter".to_string(),
            compilationMode: CompilationMode::NORMAL_COMPILE,
            systemExtDirFd: -1,
        };

        let temp_root = tempdir().unwrap();
        let authfs_mount_point = temp_root
            .path()
            .join("authfs_mount_point")
            .to_str()
            .expect("authfs mount path contains non-unicode characters)")
            .to_string();
        let result = std::fs::create_dir(&authfs_mount_point);
        assert!(result.is_ok(), "Failed to create authfs mount point: {}", result.unwrap_err());
        let staging_dir = PathBuf::from(&authfs_mount_point)
            .join(args.stagingDirFd.to_string())
            .to_str()
            .expect("authfs mount path directory contains invalid unicode characters.")
            .to_string();
        let expected_android_root =
            PathBuf::from(&authfs_mount_point).join(format!("{}/system", args.systemDirFd));

        let expected_system_properties: &str = "dalvik.vm.test";
        let _write_ctx = {
            let ctx = mock_system_properties::write_context();
            ctx.expect()
                .with(eq(expected_system_properties), eq(expected_system_properties))
                .times(1)
                .returning(|_, _| Ok(()));
            ctx
        };

        let _authfs_factory_ctx = {
            let mut authfs_svc = MockIAuthFsService::default();

            let mut authfs = MockIAuthFs::default();

            let cloned = authfs_mount_point.clone();
            authfs.expect_getMountPoint().times(1..).returning(move || Ok(cloned.clone()));
            authfs_svc
                .expect_mount()
                .withf(move |arg_in: &AuthFsConfig| {
                    arg_in.inputDirFdAnnotations.len() == 1
                        && arg_in.inputDirFdAnnotations[0].fd == args.systemDirFd
                        && arg_in.outputDirFdAnnotations.len() == 2
                        && arg_in.outputDirFdAnnotations[0].fd == args.outputDirFd
                        && arg_in.outputDirFdAnnotations[1].fd == args.stagingDirFd
                })
                .times(1)
                .return_once(move |_| Ok(Strong::new(Box::new(authfs))));
            let ctx = MockAuthFsFactory::new_authfs_service_context();
            ctx.expect().return_once(move || Ok(Strong::new(Box::new(authfs_svc))));
            ctx
        };

        let derive_cp_rv = "export DERIVE_CP_ENV_VAR VAL";
        let expected_derive_cp_env_val = "DERIVE_CP_ENV_VAR=VAL";
        let _derive_cp_ctx = {
            let ctx = mock_command_line_helper::run_derive_classpath_context();
            ctx.expect()
                .with(eq(expected_android_root))
                .returning(|_| Ok(derive_cp_rv.to_string()));
            ctx
        };

        let mock_minijail_command_tag: u32 = 12345;
        let _new_for_path_ctx = {
            let expected_odrefresh_flags: HashSet<String> = vec![
                "odrefresh".to_string(),
                "--compilation-os-mode".to_string(),
                format!("--zygote-arch={}", args.zygoteArch),
                format!("--dalvik-cache={}", args.targetDirName),
                format!("--staging-dir={}", staging_dir.to_string()),
                "--no-refresh".to_string(),
                format!("--system-server-compiler-filter={}", args.systemServerCompilerFilter),
                "--compile".to_string(),
            ]
            .into_iter()
            .collect();
            let ctx = mock_minijail_command_factory::new_for_path_context();
            ctx.expect()
                .withf(
                    move |executable: &Path,
                          keep_fds: &Vec<RawFd>,
                          args_in: &Vec<String>,
                          env_vars: &Vec<String>| {
                        executable == Path::new(ODREFRESH_PATH)
                            && keep_fds.is_empty()
                            && args_in.iter().cloned().collect::<HashSet<String>>()
                                == expected_odrefresh_flags
                            && env_vars.contains(&expected_derive_cp_env_val.to_string())
                    },
                )
                .return_once(move |_, _, _, _| {
                    Ok(mock_minijail_command {
                        real_command: None, // don't care
                        tag: mock_minijail_command_tag,
                    })
                });
            ctx
        };

        let _mock_minijail_new_ctx = {
            let mut mock_jail = MockMinijail::default();
            mock_jail
                .expect_run_command()
                .withf(move |command: &mock_minijail_command| {
                    command.tag == mock_minijail_command_tag
                })
                .returning(|_| Ok(0));
            mock_jail.expect_wait().times(1).returning(|| Ok(()));
            let ctx = MockMinijail::new_context();
            ctx.expect().return_once(|| Ok(mock_jail));
            ctx
        };

        let odrefresh_svc = CompOsService {
            odrefresh_path: PathBuf::from(ODREFRESH_PATH),
            initialized: RwLock::new(None),
        };
        assert!(odrefresh_svc
            .initializeSystemProperties(
                &[expected_system_properties.to_string()],
                &[expected_system_properties.to_string()]
            )
            .is_ok());
        assert!(odrefresh_svc.odrefresh(&args).is_ok());
    }
}
