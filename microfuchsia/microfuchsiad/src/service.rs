/*
 * Copyright (C) 2024 The Android Open Source Project
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

//! Implementation of IMicrofuchsiaService that runs microfuchsia in AVF when
//! created.

use crate::instance_manager::InstanceManager;
use crate::instance_starter::MicrofuchsiaInstance;
use android_system_microfuchsiad::aidl::android::system::microfuchsiad::IMicrofuchsiaService::{
    BnMicrofuchsiaService, IMicrofuchsiaService,
};
use anyhow::Context;
use binder::{self, BinderFeatures, Interface, Strong};

#[allow(unused)]
pub struct MicrofuchsiaService {
    instance_manager: InstanceManager,
    microfuchsia: MicrofuchsiaInstance,
}

pub fn new_binder(mut instance_manager: InstanceManager) -> Strong<dyn IMicrofuchsiaService> {
    let microfuchsia = instance_manager.start_instance().context("Starting Microfuchsia").unwrap();
    let service = MicrofuchsiaService { instance_manager, microfuchsia };
    BnMicrofuchsiaService::new_binder(service, BinderFeatures::default())
}

impl Interface for MicrofuchsiaService {}

impl IMicrofuchsiaService for MicrofuchsiaService {}
