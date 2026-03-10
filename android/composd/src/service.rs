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

//! Implementation of IIsolatedCompilationService, called from system server when compilation is
//! desired.

use crate::{
    instance_manager::IInstanceManager, odrefresh_task::OdrefreshTask,
    verified_dex2oat_task::VerifiedDex2OatTaskQueue,
};
use android_system_composd::aidl::android::system::composd::{
    ICompilationTask::{BnCompilationTask, ICompilationTask},
    ICompilationTaskCallback::ICompilationTaskCallback,
    IDex2OatTaskCallback::IDex2OatTaskCallback,
    IIsolatedCompilationService::{
        ApexSource::ApexSource, BnIsolatedCompilationService, Dex2OatArg::Dex2OatArg,
        IIsolatedCompilationService,
    },
};
use anyhow::{Context, Result};
use binder::{
    self, BinderFeatures, ExceptionCode, Interface, ParcelFileDescriptor, Status, Strong,
    ThreadState,
};
use compos_aidl_interface::aidl::com::android::compos::ICompOsService::CompilationMode::CompilationMode;
#[cfg(not(test))]
use compos_common as compos_common_injection;
#[cfg(test)]
use compos_common_with_mocks as compos_common_injection;
use log::error;
use std::time::Duration;

use compos_common_injection::{
    binder::to_binder_result,
    compos_client::CompOsType,
    odrefresh::{PENDING_ARTIFACTS_SUBDIR, TEST_ARTIFACTS_SUBDIR},
};
use rustutils::android::{users::AID_ARTD, users::AID_ROOT, users::AID_SHELL, users::AID_SYSTEM};
use std::sync::Arc;

pub struct IsolatedCompilationService {
    instance_manager: Arc<dyn IInstanceManager>,
    dex2oat_queue: Arc<VerifiedDex2OatTaskQueue>,
}

pub fn new_binder(
    instance_manager: Arc<dyn IInstanceManager>,
    dex2oat_queue: Arc<VerifiedDex2OatTaskQueue>,
) -> Strong<dyn IIsolatedCompilationService> {
    let service = IsolatedCompilationService { instance_manager, dex2oat_queue };
    BnIsolatedCompilationService::new_binder(service, BinderFeatures::default())
}

impl Interface for IsolatedCompilationService {}

impl Drop for IsolatedCompilationService {
    fn drop(&mut self) {
        self.dex2oat_queue.quit();
    }
}

impl IIsolatedCompilationService for IsolatedCompilationService {
    fn startStagedApexCompile(
        &self,
        callback: &Strong<dyn ICompilationTaskCallback>,
        base_os: &str,
    ) -> binder::Result<Strong<dyn ICompilationTask>> {
        check_permissions_for_odrefresh()?;
        to_binder_result(self.do_start_staged_apex_compile(callback, base_os))
    }

    fn startTestCompile(
        &self,
        apex_source: ApexSource,
        callback: &Strong<dyn ICompilationTaskCallback>,
        base_os: &str,
    ) -> binder::Result<Strong<dyn ICompilationTask>> {
        check_permissions_for_odrefresh()?;
        let prefer_staged = match apex_source {
            ApexSource::NoStaged => false,
            ApexSource::PreferStaged => true,
            _ => unreachable!("Invalid ApexSource {:?}", apex_source),
        };
        to_binder_result(self.do_start_test_compile(prefer_staged, callback, base_os))
    }

    fn startVerifiedDex2Oat(
        &self,
        dex2oat_args: &[Dex2OatArg],
        signed_manifest_fd: &ParcelFileDescriptor,
        results_callback: &Strong<dyn IDex2OatTaskCallback>,
        timeout_seconds: i32,
    ) -> binder::Result<Strong<dyn ICompilationTask>> {
        if !aconfig_compos_flags_rust::verified_dex2oat() {
            error!("Feature disabled.");
            return Err(Status::new_exception(ExceptionCode::UNSUPPORTED_OPERATION, None));
        }
        check_permissions_for_dex2oat()?;
        to_binder_result(self.do_start_verified_dex2oat(
            dex2oat_args,
            signed_manifest_fd,
            results_callback,
            timeout_seconds,
        ))
    }
}

impl IsolatedCompilationService {
    fn do_start_staged_apex_compile(
        &self,
        callback: &Strong<dyn ICompilationTaskCallback>,
        base_os: &str,
    ) -> Result<Strong<dyn ICompilationTask>> {
        let comp_os = self
            .instance_manager
            .start_current_instance(CompOsType::OdRefresh, base_os)
            .context("Starting CompOS for staged APEXes")?;

        let target_dir_name = PENDING_ARTIFACTS_SUBDIR.to_owned();
        let task = OdrefreshTask::start(
            comp_os,
            CompilationMode::NORMAL_COMPILE,
            target_dir_name,
            callback,
        )?;

        Ok(BnCompilationTask::new_binder(task, BinderFeatures::default()))
    }

    fn do_start_test_compile(
        &self,
        prefer_staged: bool,
        callback: &Strong<dyn ICompilationTaskCallback>,
        base_os: &str,
    ) -> Result<Strong<dyn ICompilationTask>> {
        let comp_os = self
            .instance_manager
            .start_test_instance(CompOsType::OdRefresh, prefer_staged, base_os)
            .context("Starting CompOS for test compile")?;

        let target_dir_name = TEST_ARTIFACTS_SUBDIR.to_owned();
        let task = OdrefreshTask::start(
            comp_os,
            CompilationMode::TEST_COMPILE,
            target_dir_name,
            callback,
        )?;

        Ok(BnCompilationTask::new_binder(task, BinderFeatures::default()))
    }

    fn do_start_verified_dex2oat(
        &self,
        dex2oat_args: &[Dex2OatArg],
        signed_manifest_fd: &ParcelFileDescriptor,
        callback: &Strong<dyn IDex2OatTaskCallback>,
        timeout_seconds: i32,
    ) -> Result<Strong<dyn ICompilationTask>> {
        let u_timeout: u64 = timeout_seconds
            .try_into()
            .context("Unable to convert timeout_seconds from i32 to u64")?;
        self.dex2oat_queue.enqueue_job(
            dex2oat_args,
            signed_manifest_fd,
            Duration::from_secs(u_timeout),
            callback,
        )
    }
}

fn check_permissions_for_odrefresh() -> binder::Result<()> {
    let calling_uid = ThreadState::get_calling_uid();
    // This should only be called by system server, or root while testing
    if calling_uid != AID_SYSTEM && calling_uid != AID_ROOT {
        Err(Status::new_exception(ExceptionCode::SECURITY, None))
    } else {
        Ok(())
    }
}

fn check_permissions_for_dex2oat() -> binder::Result<()> {
    let calling_uid = ThreadState::get_calling_uid();
    // restrict to ARTd, shell (for testing) and root.
    match calling_uid {
        AID_ARTD | AID_SHELL | AID_ROOT => Ok(()),
        _ => Err(Status::new_exception(ExceptionCode::SECURITY, None)),
    }
}
