/*
 * Copyright 2021 The Android Open Source Project
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

//! Handle running odrefresh in the VM, with an async interface to allow cancellation

#[cfg(not(test))]
use {
    self::helper::*,
    crate::{fd_server_helper::FdServer, wrappers::composd_native},
    compos_wrappers::{fsverity, paths, system_properties},
};
#[cfg(test)]
use {
    self::mock_helper::*,
    crate::{
        fd_server_helper::MockFdServer as FdServer, wrappers::mock_composd_native as composd_native,
    },
    compos_wrappers_with_mocks::{
        mock_fsverity as fsverity, mock_paths as paths, mock_system_properties as system_properties,
    },
};

use crate::fd_server_helper::FdServerConfig;
use crate::instance_starter::CompOsInstance;
use crate::wrappers::compos_common_injection;
use android_system_composd::aidl::android::system::composd::{
    ICompilationTask::ICompilationTask,
    ICompilationTaskCallback::{FailureReason::FailureReason, ICompilationTaskCallback},
};
use anyhow::{bail, Context, Result};
use binder::{Interface, Result as BinderResult, Strong};
use compos_aidl_interface::aidl::com::android::compos::ICompOsService::{
    CompilationMode::CompilationMode, ICompOsService, OdrefreshArgs::OdrefreshArgs,
};
use compos_common_injection::{
    compos_client::CompOsService,
    odrefresh::{
        is_system_property_interesting, ExitCode, CURRENT_ARTIFACTS_SUBDIR,
        ODREFRESH_OUTPUT_ROOT_DIR, PENDING_ARTIFACTS_SUBDIR,
    },
    BUILD_MANIFEST_SYSTEM_EXT_APK_PATH,
};
use log::{error, info, warn};
use odsign_proto::odsign_info::OdsignInfo;
use protobuf::Message;
use std::fs::{remove_dir_all, File, OpenOptions};
use std::os::fd::AsFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, OwnedFd};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone)]
pub struct OdrefreshTask {
    running_task: Arc<Mutex<Option<RunningTask>>>,
}

impl Interface for OdrefreshTask {}

impl ICompilationTask for OdrefreshTask {
    fn cancel(&self) -> BinderResult<()> {
        let task = self.take();
        // Drop the VM, which should end compilation - and cause our thread to exit.
        // Note that we don't do a graceful shutdown here; we've been asked to give up our resources
        // ASAP, and the VM has not failed so we don't need to ensure VM logs are written.
        drop(task);
        Ok(())
    }
}

struct RunningTask {
    callback: Strong<dyn ICompilationTaskCallback>,
    #[allow(dead_code)] // Keeps the CompOS VM alive
    comp_os: CompOsInstance,
}

impl OdrefreshTask {
    /// Return the current running task, if any, removing it from this CompilationTask.
    /// Once removed, meaning the task has ended or been canceled, further calls will always return
    /// None.
    fn take(&self) -> Option<RunningTask> {
        self.running_task.lock().unwrap().take()
    }

    pub fn start(
        comp_os: CompOsInstance,
        compilation_mode: CompilationMode,
        target_dir_name: String,
        callback: &Strong<dyn ICompilationTaskCallback>,
    ) -> Result<OdrefreshTask> {
        let service = match comp_os.get_service() {
            CompOsService::OdRefresh(s) => s,
            _ => bail!("Tried starting when underlying VM does not provide an OdRefresh service."),
        };
        let task = RunningTask { comp_os, callback: callback.clone() };
        let task = OdrefreshTask { running_task: Arc::new(Mutex::new(Some(task))) };

        task.clone().start_thread(service, compilation_mode, target_dir_name);

        Ok(task)
    }

    fn start_thread(
        self,
        service: Strong<dyn ICompOsService>,
        compilation_mode: CompilationMode,
        target_dir_name: String,
    ) {
        thread::spawn(move || {
            let exit_code = run_in_vm(service, compilation_mode, &target_dir_name);

            let task = self.take();
            // We don't do the callback if cancel has already happened.
            if let Some(RunningTask { callback, comp_os }) = task {
                // Make sure we keep our service alive until we have called the callback.
                let lazy_service_guard = comp_os.shutdown();

                let result = match exit_code {
                    Ok(ExitCode::CompilationSuccess) => {
                        if compilation_mode == CompilationMode::TEST_COMPILE {
                            info!("Compilation success");
                            callback.onSuccess()
                        } else {
                            // compos.info is generated only during NORMAL_COMPILE
                            if let Err(e) = enable_fsverity_to_all() {
                                let message =
                                    format!("Unexpected failure when enabling fs-verity: {e:?}");
                                error!("{message}");
                                callback.onFailure(FailureReason::FailedToEnableFsverity, &message)
                            } else {
                                info!("Compilation success, fs-verity enabled");
                                callback.onSuccess()
                            }
                        }
                    }
                    Ok(exit_code) => {
                        let message = format!("Unexpected odrefresh result: {exit_code:?}");
                        error!("{message}");
                        callback.onFailure(FailureReason::UnexpectedCompilationResult, &message)
                    }
                    Err(e) => {
                        let message = format!("Running odrefresh failed: {e:?}");
                        error!("{message}");
                        callback.onFailure(FailureReason::CompilationFailed, &message)
                    }
                };
                if let Err(e) = result {
                    warn!("Failed to deliver callback: {e:?}");
                }
                drop(lazy_service_guard);
            }
        });
    }
}

/// Returns an `OwnedFD` of the directory.
fn open_dir(path: &Path) -> Result<OwnedFd> {
    Ok(OwnedFd::from(
        OpenOptions::new()
            .custom_flags(libc::O_DIRECTORY)
            .read(true) // O_DIRECTORY can only be opened with read
            .open(path)
            .with_context(|| format!("Failed to open {path:?} directory as path fd"))?,
    ))
}

#[cfg_attr(test, mockall::automock, allow(dead_code))]
mod helper {
    use super::*;
    pub fn run_in_vm(
        service: Strong<dyn ICompOsService>,
        compilation_mode: CompilationMode,
        target_dir_name: &str,
    ) -> Result<ExitCode> {
        let mut names = Vec::new();
        let mut values = Vec::new();
        system_properties::foreach(|name, value| {
            if is_system_property_interesting(name) {
                names.push(name.to_owned());
                values.push(value.to_owned());
            }
        })?;
        service
            .initializeSystemProperties(&names, &values)
            .context("initialize system properties")?;

        let output_root = paths::root_rebase(ODREFRESH_OUTPUT_ROOT_DIR);

        // We need to remove the target directory because odrefresh running in compos will create it
        // (and can't see the existing one, since authfs doesn't show it existing files in an output
        // directory).
        let target_path = output_root.as_path().join(target_dir_name);
        if target_path.exists() {
            remove_dir_all(&target_path)
                .with_context(|| format!("Failed to delete {}", target_path.display()))?;
        }

        let staging_dir_fd =
            open_dir(composd_native::palette_create_odrefresh_staging_directory()?)?;
        let system_dir_fd = open_dir(&paths::root_rebase("/system"))?;
        let output_dir_fd = open_dir(&output_root)?;

        // Get the raw FD before passing the ownership, since borrowing will violate the borrow
        // check.
        let system_dir_raw_fd = system_dir_fd.as_raw_fd();
        let output_dir_raw_fd = output_dir_fd.as_raw_fd();
        let staging_dir_raw_fd = staging_dir_fd.as_raw_fd();

        // When the VM starts, it starts with or without mouting the extra build manifest APK from
        // /system_ext. Later on request (here), we need to pass the directory FD of /system_ext,
        // but only if the VM is configured to need it.
        //
        // It is possible to plumb the information from ComposClient to here, but it's extra
        // complexity and feel slightly weird to encode the VM's state to the task itself,
        // as it is a request to the VM.
        let need_system_ext = paths::root_rebase(BUILD_MANIFEST_SYSTEM_EXT_APK_PATH).exists();
        let (system_ext_dir_raw_fd, ro_dir_fds) = if need_system_ext {
            let system_ext_dir_fd = open_dir(paths::root_rebase("/system_ext").as_path())?;
            (system_ext_dir_fd.as_raw_fd(), vec![system_dir_fd, system_ext_dir_fd])
        } else {
            (-1, vec![system_dir_fd])
        };

        // Spawn a fd_server to serve the FDs.
        let fd_server_config = FdServerConfig {
            ro_dir_fds,
            rw_dir_fds: vec![staging_dir_fd, output_dir_fd],
            ..Default::default()
        };

        let fd_server_raii = FdServer::build_from_config(fd_server_config)?;

        let zygote_arch = system_properties::read("ro.zygote")?.context("ro.zygote not set")?;
        let system_server_compiler_filter =
            system_properties::read("dalvik.vm.systemservercompilerfilter")?.unwrap_or_default();

        let args = OdrefreshArgs {
            compilationMode: compilation_mode,
            systemDirFd: system_dir_raw_fd,
            systemExtDirFd: system_ext_dir_raw_fd,
            outputDirFd: output_dir_raw_fd,
            stagingDirFd: staging_dir_raw_fd,
            targetDirName: target_dir_name.to_string(),
            zygoteArch: zygote_arch,
            systemServerCompilerFilter: system_server_compiler_filter,
        };
        let exit_code = service.odrefresh(&args)?;
        drop(fd_server_raii);
        ExitCode::from_i32(exit_code.into())
    }

    /// Enable fs-verity to output artifacts according to compos.info in the pending directory. Any
    /// error before the completion will just abort, leaving the previous files enabled.
    pub fn enable_fsverity_to_all() -> Result<()> {
        let odrefresh_current_dir =
            paths::root_rebase(ODREFRESH_OUTPUT_ROOT_DIR).join(CURRENT_ARTIFACTS_SUBDIR);
        let pending_dir =
            paths::root_rebase(ODREFRESH_OUTPUT_ROOT_DIR).join(PENDING_ARTIFACTS_SUBDIR);
        let mut reader =
            File::open(pending_dir.join("compos.info")).context("Failed to open compos.info")?;
        let compos_info = OdsignInfo::parse_from_reader(&mut reader).context("Failed to parse")?;

        for path_str in compos_info.file_hashes.keys() {
            // Need to rebase the directory on to compos-pending first
            if let Ok(relpath) = Path::new(path_str).strip_prefix(&odrefresh_current_dir) {
                let path = pending_dir.join(relpath);
                let file =
                    File::open(&path).with_context(|| format!("Failed to open {:?}", path))?;
                // We don't expect error. But when it happens, don't bother handle it here. For
                // simplicity, just let odsign do the regular check.
                fsverity::enable(file.as_fd())
                    .with_context(|| format!("Failed to enable fs-verity to {:?}", path))?;
            } else {
                warn!("Skip due to unexpected path: {}", path_str);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::{helper, mock_helper, OdrefreshTask};
    use crate::{
        fd_server_helper::{FdServerConfig, MockFdServer},
        instance_starter::CompOsInstance,
        test_util::{dir::*, fd, sync::CompletionBarrier},
        wrappers::{binder::MockLazyServiceGuard, mock_composd_native},
    };
    use anyhow::Error;
    use binder::BinderFeatures;
    use compos_aidl_interface::aidl::com::android::compos::ICompOsService::{
        BnCompOsService, CompilationMode::CompilationMode, MockICompOsService,
        OdrefreshArgs::OdrefreshArgs,
    };
    use compos_common_with_mocks::{
        binder::to_binder_result,
        compos_client::{CompOsService, MockComposClient},
        odrefresh::{ExitCode, ODREFRESH_OUTPUT_ROOT_DIR, PENDING_ARTIFACTS_SUBDIR},
    };
    use compos_wrappers_with_mocks::{mock_paths, mock_system_properties};
    use mockall::predicate::{always as any, eq};
    use once_cell::sync::Lazy;
    use std::{
        collections::{HashMap, HashSet},
        os::fd::{AsRawFd, OwnedFd},
        path::{Path, PathBuf},
        time::Duration,
    };
    use tempfile::{tempdir, TempDir};

    use android_system_composd::aidl::android::system::composd::ICompilationTaskCallback::{
        BnCompilationTaskCallback, MockICompilationTaskCallback,
    };

    const ALLOWLISTED_PROPERTIES: [(&str, &str); 3] = [
        ("dalvik.vm.PROP1", "VAL1"),
        ("ro.dalvik.vm.PROP2", "VAL2"),
        ("persist.device_config.runtime_native_boot.PROP3", "VAL3"),
    ];
    // Properties that do not begin with allow-listed prefixes.
    const FILTERED_OUT_PROPERTIES: [(&str, &str); 3] =
        [("BAD_PROP1", "VAL1"), ("BAD_PROP2", "VAL2"), ("BAD_PROP3", "VAL3")];

    const DEFAULT_BCC: &[u8] = b"DEFAULT_BCC";
    const PROPERTY_ZYGOTE_ARCH: &str = "ZYGOTE_ARCH";
    const PROPERTY_SS_COMPILER_FILTER: &str = "SYSTEM_SERVER_COMPILER_FILTER";

    fn get_staging_subdir(root_path: &Path) -> PathBuf {
        rebase_subdir(root_path, STAGING_SUBDIR)
    }
    fn get_odrefresh_output_dir(root_path: &Path) -> PathBuf {
        rebase_subdir(root_path, ODREFRESH_OUTPUT_ROOT_DIR)
    }

    #[test]
    fn run_in_vm_normal_compile() {
        static ROOT_DIR: Lazy<TempDir> = Lazy::new(|| tempdir().unwrap());
        static STAGING_DIR_PATHBUF: Lazy<PathBuf> =
            Lazy::new(|| ROOT_DIR.path().join(STAGING_SUBDIR));
        for dir in [SYSTEM_SUBDIR, STAGING_SUBDIR, ODREFRESH_OUTPUT_ROOT_DIR].iter() {
            if let Err(e) = create_subdir(ROOT_DIR.path(), dir) {
                panic!("Test setup failed: {}", e)
            }
        }
        let root_rebase_ctx = mock_paths::root_rebase_context();
        root_rebase_ctx
            .expect()
            .withf(|frag: &str| frag.starts_with("/"))
            .returning(|frag: &str| ROOT_DIR.path().join(frag.strip_prefix("/").unwrap_or(frag)));
        let palette_create_ctx =
            mock_composd_native::palette_create_odrefresh_staging_directory_context();
        let system_properties_read_ctx = mock_system_properties::read_context();
        let fd_server_build_from_config_ctx = MockFdServer::build_from_config_context();
        let system_properties_for_each_ctx = mock_system_properties::foreach_context();

        let mock_compos_svc = {
            let mut mock = MockICompOsService::default();
            mock.expect_getAttestationChain()
                .returning(|| to_binder_result::<Vec<u8>, Error>(Ok(DEFAULT_BCC.to_vec())));

            system_properties_for_each_ctx.expect().returning(
                |mut closure: Box<dyn for<'a, 'b> FnMut(&'a str, &'b str)>| {
                    // Properties that begin with allow listed prefixes.
                    ALLOWLISTED_PROPERTIES.iter().for_each(|(k, v)| closure(k, v));
                    // Properties that do not begin with allow listed prefixes.
                    FILTERED_OUT_PROPERTIES.iter().for_each(|(k, v)| closure(k, v));
                    Ok(())
                },
            );
            mock.expect_initializeSystemProperties()
                .withf(|k, v| {
                    let in_set: HashMap<String, String> = k
                        .iter()
                        .zip(v.iter())
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    let expected_set: HashMap<String, String> = ALLOWLISTED_PROPERTIES
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    in_set == expected_set
                })
                .return_once(|_, _| Ok(()));
            palette_create_ctx.expect().returning(|| Ok(&STAGING_DIR_PATHBUF));
            system_properties_read_ctx
                .expect()
                .with(eq("ro.zygote"))
                .return_once(|_| Ok(Some(PROPERTY_ZYGOTE_ARCH.to_string())));
            system_properties_read_ctx
                .expect()
                .with(eq("dalvik.vm.systemservercompilerfilter"))
                .return_once(|_| Ok(Some(PROPERTY_SS_COMPILER_FILTER.to_owned())));
            fd_server_build_from_config_ctx
                .expect()
                .withf(|cfg: &FdServerConfig| {
                    let rw_dir_paths: HashSet<PathBuf> = cfg
                        .rw_dir_fds
                        .iter()
                        .map(|fd: &OwnedFd| fd::get_path(&fd.as_raw_fd()))
                        .collect();
                    let ro_dir_paths: HashSet<PathBuf> = cfg
                        .ro_dir_fds
                        .iter()
                        .map(|fd: &OwnedFd| fd::get_path(&fd.as_raw_fd()))
                        .collect();
                    let expected_rw_dir_paths = HashSet::from([
                        get_staging_subdir(ROOT_DIR.path()),
                        get_odrefresh_output_dir(ROOT_DIR.path()),
                    ]);
                    let expected_ro_dir_paths = HashSet::from([get_system_subdir(ROOT_DIR.path())]);
                    expected_rw_dir_paths == rw_dir_paths
                        && expected_ro_dir_paths == ro_dir_paths
                        && cfg.ro_file_fds.is_empty()
                        && cfg.rw_file_fds.is_empty()
                })
                .return_once(|_| {
                    let mut mock_fd_server = MockFdServer::new();
                    mock_fd_server.expect_drop().times(1).return_const(());
                    Ok(mock_fd_server)
                });
            mock.expect_odrefresh()
                .withf(|args: &OdrefreshArgs| {
                    args.compilationMode == CompilationMode::NORMAL_COMPILE
                        && args.zygoteArch == PROPERTY_ZYGOTE_ARCH
                        && args.systemServerCompilerFilter == PROPERTY_SS_COMPILER_FILTER
                        && args.targetDirName == PENDING_ARTIFACTS_SUBDIR
                })
                .return_once(|_| Ok(ExitCode::CompilationSuccess as i8));
            mock.expect_quit().return_once(|| Ok(()));
            BnCompOsService::new_binder(mock, BinderFeatures::default())
        };
        let result = helper::run_in_vm(
            mock_compos_svc,
            CompilationMode::NORMAL_COMPILE,
            PENDING_ARTIFACTS_SUBDIR,
        );
        assert!(result.is_ok());
        let exit_code = result.unwrap();
        assert!(exit_code == ExitCode::CompilationSuccess);
    }
    #[test]
    fn odrefresh_task_normal_compile() {
        static ROOT_DIR: Lazy<TempDir> = Lazy::new(|| tempdir().unwrap());
        for dir in [SYSTEM_SUBDIR, STAGING_SUBDIR, ODREFRESH_OUTPUT_ROOT_DIR].iter() {
            if let Err(e) = create_subdir(ROOT_DIR.path(), dir) {
                panic!("Test setup failed: {}", e)
            }
        }
        let root_rebase_ctx = mock_paths::root_rebase_context();
        root_rebase_ctx
            .expect()
            .withf(|frag: &str| frag.starts_with("/"))
            .returning(|frag: &str| ROOT_DIR.path().join(frag.strip_prefix("/").unwrap_or(frag)));

        let mock_compos_client = {
            let mut mock = MockComposClient::default();
            mock.expect_shutdown()
                .withf(|s| matches!(s, CompOsService::OdRefresh(_)))
                .return_once(|_| {});
            mock
        };
        let mut mock_lazy_service_guard = MockLazyServiceGuard::default();
        mock_lazy_service_guard.expect_drop().times(1).return_const(());

        let compos_instance = {
            let mock = MockICompOsService::default();
            let bn = BnCompOsService::new_binder(mock, BinderFeatures::default());
            let compos_svc = CompOsService::OdRefresh(bn);
            CompOsInstance::new_for_test(mock_compos_client, compos_svc, mock_lazy_service_guard)
        };
        let completion_barrier = CompletionBarrier::new();
        let callback = {
            let mut mock = MockICompilationTaskCallback::new();
            let on_success_completion = completion_barrier.clone();
            mock.expect_onSuccess().return_once(move || {
                on_success_completion.mark_completed();
                Ok(())
            });
            let on_failure_completion = completion_barrier.clone();
            mock.expect_onFailure().never().return_once(move |_, _| {
                on_failure_completion.mark_completed();
                Ok(())
            });
            BnCompilationTaskCallback::new_binder(mock, BinderFeatures::default())
        };
        let run_in_vm_ctx = mock_helper::run_in_vm_context();
        run_in_vm_ctx
            .expect()
            .with(any(), eq(CompilationMode::NORMAL_COMPILE), eq(PENDING_ARTIFACTS_SUBDIR))
            .return_once(|_, _, _| Ok(ExitCode::CompilationSuccess));
        let enable_fsverity_to_all_ctx = mock_helper::enable_fsverity_to_all_context();
        enable_fsverity_to_all_ctx.expect().times(1).return_once(|| Ok(()));

        let target_dir_name = PENDING_ARTIFACTS_SUBDIR.to_owned();
        let odrefresh_task = OdrefreshTask::start(
            compos_instance,
            CompilationMode::NORMAL_COMPILE,
            target_dir_name,
            &callback,
        );
        // Wait until callback.OnSuccess,onFailure is called or 1 second elapses, whichever comes
        // first.
        let _ = completion_barrier.wait_for_completion(Duration::from_secs(1));
        assert!(odrefresh_task.is_ok());
    }
}
