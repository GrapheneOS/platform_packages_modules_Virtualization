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

use crate::instance_manager::InstanceManager;
use crate::odrefresh_task::OdrefreshTask;
use android_system_composd::aidl::android::system::composd::{
    ICompilationTask::{BnCompilationTask, ICompilationTask},
    ICompilationTaskCallback::ICompilationTaskCallback,
    IIsolatedCompilationService::{
        ApexSource::ApexSource, BnIsolatedCompilationService, IIsolatedCompilationService,
    },
};
use anyhow::{Context, Result};
use binder::{self, BinderFeatures, ExceptionCode, Interface, Status, Strong, ThreadState};
use compos_aidl_interface::aidl::com::android::compos::ICompOsService::CompilationMode::CompilationMode;
use compos_common::binder::to_binder_result;
use compos_common::odrefresh::{PENDING_ARTIFACTS_SUBDIR, TEST_ARTIFACTS_SUBDIR};
use rustutils::{users::AID_ROOT, users::AID_SYSTEM};
use std::sync::Arc;

pub struct IsolatedCompilationService {
    instance_manager: Arc<InstanceManager>,
}

pub fn new_binder(
    instance_manager: Arc<InstanceManager>,
) -> Strong<dyn IIsolatedCompilationService> {
    let service = IsolatedCompilationService { instance_manager };
    BnIsolatedCompilationService::new_binder(service, BinderFeatures::default())
}

impl Interface for IsolatedCompilationService {}

impl IIsolatedCompilationService for IsolatedCompilationService {
    fn startStagedApexCompile(
        &self,
        callback: &Strong<dyn ICompilationTaskCallback>,
    ) -> binder::Result<Strong<dyn ICompilationTask>> {
        check_permissions()?;
        to_binder_result(self.do_start_staged_apex_compile(callback))
    }

    fn startTestCompile(
        &self,
        apex_source: ApexSource,
        callback: &Strong<dyn ICompilationTaskCallback>,
    ) -> binder::Result<Strong<dyn ICompilationTask>> {
        check_permissions()?;
        let prefer_staged = match apex_source {
            ApexSource::NoStaged => false,
            ApexSource::PreferStaged => true,
            _ => unreachable!("Invalid ApexSource {:?}", apex_source),
        };
        to_binder_result(self.do_start_test_compile(prefer_staged, callback))
    }
}

impl IsolatedCompilationService {
    fn do_start_staged_apex_compile(
        &self,
        callback: &Strong<dyn ICompilationTaskCallback>,
    ) -> Result<Strong<dyn ICompilationTask>> {
        let comp_os = self.instance_manager.start_current_instance().context("Starting CompOS")?;

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
    ) -> Result<Strong<dyn ICompilationTask>> {
        let comp_os =
            self.instance_manager.start_test_instance(prefer_staged).context("Starting CompOS")?;

        let target_dir_name = TEST_ARTIFACTS_SUBDIR.to_owned();
        let task = OdrefreshTask::start(
            comp_os,
            CompilationMode::TEST_COMPILE,
            target_dir_name,
            callback,
        )?;

        Ok(BnCompilationTask::new_binder(task, BinderFeatures::default()))
    }
}

fn check_permissions() -> binder::Result<()> {
    let calling_uid = ThreadState::get_calling_uid();
    // This should only be called by system server, or root while testing
    if calling_uid != AID_SYSTEM && calling_uid != AID_ROOT {
        Err(Status::new_exception(ExceptionCode::SECURITY, None))
    } else {
        Ok(())
    }
}
