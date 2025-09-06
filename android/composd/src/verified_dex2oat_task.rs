/*
 * Copyright 2025 The Android Open Source Project
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

//! Handle running dex2oat in the VM, with an async interface to allow cancellation
use android_system_composd::aidl::android::system::composd::ICompilationTask::ICompilationTask;
use binder::{Interface, Result as BinderResult};

#[derive(Clone)]
pub struct VerifiedDex2OatTask {}

impl Interface for VerifiedDex2OatTask {}

impl ICompilationTask for VerifiedDex2OatTask {
    fn cancel(&self) -> BinderResult<()> {
        todo!("b415850856: implementation needed");
    }
}
