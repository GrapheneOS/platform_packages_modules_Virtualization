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

//! Manages running instances of the CompOS VM. At most one instance should be running at
//! a time, started on demand.

use crate::instance_starter::{CompOsInstance, InstanceStarter};
use android_system_virtualizationservice::aidl::android::system::virtualizationservice;
use anyhow::{anyhow, bail, Context, Result};
use binder::Strong;
use compos_common::compos_client::{VmCpuTopology, VmParameters};
use compos_common::{CURRENT_INSTANCE_DIR, TEST_INSTANCE_DIR};
use log::info;
use rustutils::system_properties;
use std::str::FromStr;
use std::sync::{Arc, Mutex, Weak};
use virtualizationservice::IVirtualizationService::IVirtualizationService;

pub struct InstanceManager {
    service: Strong<dyn IVirtualizationService>,
    state: Mutex<State>,
}

impl InstanceManager {
    pub fn new(service: Strong<dyn IVirtualizationService>) -> Self {
        Self { service, state: Default::default() }
    }

    pub fn start_current_instance(&self) -> Result<CompOsInstance> {
        let mut vm_parameters = new_vm_parameters()?;
        vm_parameters.name = String::from("Composd");
        vm_parameters.prefer_staged = true;
        self.start_instance(CURRENT_INSTANCE_DIR, vm_parameters)
    }

    pub fn start_test_instance(&self, prefer_staged: bool) -> Result<CompOsInstance> {
        let mut vm_parameters = new_vm_parameters()?;
        vm_parameters.name = String::from("ComposdTest");
        vm_parameters.debug_mode = true;
        vm_parameters.prefer_staged = prefer_staged;
        self.start_instance(TEST_INSTANCE_DIR, vm_parameters)
    }

    fn start_instance(
        &self,
        instance_name: &str,
        vm_parameters: VmParameters,
    ) -> Result<CompOsInstance> {
        let mut state = self.state.lock().unwrap();
        state.mark_starting()?;
        // Don't hold the lock while we start the instance to avoid blocking other callers.
        drop(state);

        let instance_starter = InstanceStarter::new(instance_name, vm_parameters);
        let instance = instance_starter.start_new_instance(&*self.service);

        let mut state = self.state.lock().unwrap();
        if let Ok(ref instance) = instance {
            state.mark_started(instance.get_instance_tracker())?;
        } else {
            state.mark_stopped();
        }
        instance
    }
}

fn new_vm_parameters() -> Result<VmParameters> {
    // By default, dex2oat starts as many threads as there are CPUs. This can be overridden with
    // a system property. Start the VM with all CPUs and assume the guest will start a suitable
    // number of dex2oat threads.
    let cpu_topology = VmCpuTopology::MatchHost;
    let memory_mib = Some(compos_memory_mib()?);
    Ok(VmParameters { cpu_topology, memory_mib, ..Default::default() })
}

fn compos_memory_mib() -> Result<i32> {
    // Enough memory to complete odrefresh in the VM, for older versions of ART that don't set the
    // property explicitly.
    const DEFAULT_MEMORY_MIB: u32 = 600;

    let art_requested_mib =
        read_property("composd.vm.art.memory_mib.config")?.unwrap_or(DEFAULT_MEMORY_MIB);

    let vm_adjustment_mib = read_property("composd.vm.vendor.memory_mib.config")?.unwrap_or(0);

    info!(
        "Compilation VM memory: ART requests {art_requested_mib} MiB, \
        VM adjust is {vm_adjustment_mib}"
    );
    art_requested_mib
        .checked_add_signed(vm_adjustment_mib)
        .and_then(|x| x.try_into().ok())
        .context("Invalid vm memory adjustment")
}

fn read_property<T: FromStr>(name: &str) -> Result<Option<T>> {
    let str = system_properties::read(name).context("Failed to read {name}")?;
    str.map(|s| s.parse().map_err(|_| anyhow!("Invalid {name}: {s}"))).transpose()
}

// Ensures we only run one instance at a time.
// Valid states:
// Starting: is_starting is true, instance_tracker is None.
// Started: is_starting is false, instance_tracker is Some(x) and there is a strong ref to x.
// Stopped: is_starting is false and instance_tracker is None or a weak ref to a dropped instance.
// The panic calls here should never happen, unless the code above in InstanceManager is buggy.
// In particular nothing the client does should be able to trigger them.
#[derive(Default)]
struct State {
    instance_tracker: Option<Weak<()>>,
    is_starting: bool,
}

impl State {
    // Move to Starting iff we are Stopped.
    fn mark_starting(&mut self) -> Result<()> {
        if self.is_starting {
            bail!("An instance is already starting");
        }
        if let Some(weak) = &self.instance_tracker {
            if weak.strong_count() != 0 {
                bail!("An instance is already running");
            }
        }
        self.instance_tracker = None;
        self.is_starting = true;
        Ok(())
    }

    // Move from Starting to Stopped.
    fn mark_stopped(&mut self) {
        if !self.is_starting || self.instance_tracker.is_some() {
            panic!("Tried to mark stopped when not starting");
        }
        self.is_starting = false;
    }

    // Move from Starting to Started.
    fn mark_started(&mut self, instance_tracker: &Arc<()>) -> Result<()> {
        if !self.is_starting {
            panic!("Tried to mark started when not starting")
        }
        if self.instance_tracker.is_some() {
            panic!("Attempted to mark started when already started");
        }
        self.is_starting = false;
        self.instance_tracker = Some(Arc::downgrade(instance_tracker));
        Ok(())
    }
}
