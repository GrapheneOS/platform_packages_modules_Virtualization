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

//! Manages running instances of the Microfuchsia VM.
//! At most one instance should be running at a time.

use crate::instance_starter::{InstanceStarter, MicrofuchsiaInstance};
use android_system_virtualizationservice::aidl::android::system::virtualizationservice;
use anyhow::{bail, Result};
use binder::Strong;
use virtualizationservice::IVirtualizationService::IVirtualizationService;

pub struct InstanceManager {
    service: Strong<dyn IVirtualizationService>,
    started: bool,
}

impl InstanceManager {
    pub fn new(service: Strong<dyn IVirtualizationService>) -> Self {
        Self { service, started: false }
    }

    pub fn start_instance(&mut self) -> Result<MicrofuchsiaInstance> {
        if self.started {
            bail!("Cannot start multiple microfuchsia instances");
        }

        let instance_starter = InstanceStarter::new("Microfuchsia", 0);
        let instance = instance_starter.start_new_instance(&*self.service);

        if instance.is_ok() {
            self.started = true;
        }
        instance
    }
}
