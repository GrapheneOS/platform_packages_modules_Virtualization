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

use crate::wrappers::compos_common_injection;

#[cfg(not(test))]
use {crate::instance_starter::InstanceStarter, compos_wrappers::system_properties};
#[cfg(test)]
use {
    crate::instance_starter::MockInstanceStarter as InstanceStarter,
    compos_wrappers_with_mocks::mock_system_properties as system_properties,
};

use crate::instance_starter::CompOsInstance;
use android_system_virtualizationservice::aidl::android::system::virtualizationservice;
use anyhow::{anyhow, bail, Context, Result};
use binder::Strong;

use compos_common_injection::{
    compos_client::{CompOsType, VmCpuTopology, VmParameters},
    CURRENT_INSTANCE_DIR, DEX2OAT_INSTANCE_DIR, TEST_INSTANCE_DIR,
};

use log::info;
use std::str::FromStr;
use std::sync::{Arc, Mutex, Weak};
use virtualizationservice::IVirtualizationService::IVirtualizationService;

#[cfg_attr(test, mockall::automock)]
pub trait IInstanceManager: Send + Sync {
    fn start_current_instance(
        &self,
        compos_type: CompOsType,
        base_os: &str,
    ) -> Result<CompOsInstance>;

    fn start_test_instance(
        &self,
        compos_type: CompOsType,
        prefer_staged: bool,
        base_os: &str,
    ) -> Result<CompOsInstance>;
}

pub struct InstanceManager {
    service: Strong<dyn IVirtualizationService>,
    odrefresh_vm_state: Mutex<State>,
    dex2oat_vm_state: Mutex<State>,
}

impl IInstanceManager for InstanceManager {
    fn start_current_instance(
        &self,
        compos_type: CompOsType,
        base_os: &str,
    ) -> Result<CompOsInstance> {
        let name = match compos_type {
            CompOsType::OdRefresh => "VerifiedOdRefresh".to_owned(),
            CompOsType::Dex2Oat => "VerifiedDex2Oat".to_owned(),
        };
        let instance_name = match compos_type {
            CompOsType::OdRefresh => CURRENT_INSTANCE_DIR,
            CompOsType::Dex2Oat => DEX2OAT_INSTANCE_DIR,
        };
        let vm_parameters = new_vm_parameters(name, compos_type, base_os)?;
        self.start_instance(instance_name, vm_parameters)
    }

    fn start_test_instance(
        &self,
        compos_type: CompOsType,
        prefer_staged: bool,
        base_os: &str,
    ) -> Result<CompOsInstance> {
        let name = match compos_type {
            CompOsType::OdRefresh => "VerifiedOdRefreshTest".to_owned(),
            CompOsType::Dex2Oat => "VerifiedDex2OatTest".to_owned(),
        };
        let mut vm_parameters = new_vm_parameters(name, compos_type, base_os)?;
        vm_parameters.debug_mode = true;
        vm_parameters.prefer_staged = prefer_staged;
        self.start_instance(TEST_INSTANCE_DIR, vm_parameters)
    }
}

impl InstanceManager {
    pub fn new(service: Strong<dyn IVirtualizationService>) -> Self {
        Self {
            service,
            odrefresh_vm_state: Default::default(),
            dex2oat_vm_state: Default::default(),
        }
    }

    fn start_instance(
        &self,
        instance_name: &str,
        vm_parameters: VmParameters,
    ) -> Result<CompOsInstance> {
        let state_mutex = match vm_parameters.compos_type {
            CompOsType::OdRefresh => &self.odrefresh_vm_state,
            CompOsType::Dex2Oat => &self.dex2oat_vm_state,
        };
        let mut state_guard = state_mutex.lock().unwrap();
        state_guard.mark_starting()?;
        // Don't hold the lock while we start the instance to avoid blocking other callers.
        drop(state_guard);

        let instance_starter = InstanceStarter::new(instance_name, vm_parameters);
        let instance = instance_starter.start_new_instance(&*self.service);

        state_guard = state_mutex.lock().unwrap();
        if let Ok(ref instance) = instance {
            state_guard.mark_started(instance.get_instance_tracker())?;
        } else {
            state_guard.mark_stopped();
        }
        instance
    }
}

fn new_vm_parameters(name: String, compos_type: CompOsType, base_os: &str) -> Result<VmParameters> {
    // By default, dex2oat starts as many threads as there are CPUs. This can be overridden with
    // a system property. Start the VM with all CPUs and assume the guest will start a suitable
    // number of dex2oat threads.
    let cpu_topology = VmCpuTopology::MatchHost;
    let memory_mib = Some(compos_memory_mib()?);
    let base_os = base_os.to_owned();
    let vm_param = VmParameters {
        name,
        base_os,
        cpu_topology,
        memory_mib,
        prefer_staged: match compos_type {
            CompOsType::Dex2Oat => false,
            CompOsType::OdRefresh => true,
        },
        compos_type,
        debug_mode: bool::default(),
    };
    Ok(vm_param)
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

#[cfg(test)]
mod test {
    use super::{IInstanceManager, InstanceManager};
    use crate::{
        instance_starter::{CompOsInstance, MockInstanceStarter},
        wrappers::binder::MockLazyServiceGuard,
    };
    use compos_common_with_mocks::{
        compos_client::{CompOsService, CompOsType, MockComposClient, VmCpuTopology, VmParameters},
        CURRENT_INSTANCE_DIR,
    };
    use compos_wrappers_with_mocks::mock_system_properties;
    use mockall::predicate::{always as any, eq};
    use binder::BinderFeatures;
    use android_system_virtualizationservice::aidl::android::system::virtualizationservice::
        IVirtualizationService::{BnVirtualizationService, MockIVirtualizationService} ;
    use compos_aidl_interface::aidl::com::android::compos::ICompOsService::{
        BnCompOsService, MockICompOsService,
    };

    const BASE_OS: &str = "microdroid";
    const ART_MEMORY_PROPERTY: &str = "composd.vm.art.memory_mib.config";
    const VENDOR_MEMORY_PROPERTY: &str = "composd.vm.vendor.memory_mib.config";

    #[test]
    pub fn instance_manager_odrefresh_vm_start_success() {
        let expected_vm_param = VmParameters {
            name: "VerifiedOdRefresh".to_owned(),
            base_os: BASE_OS.to_owned(),
            debug_mode: false,
            cpu_topology: VmCpuTopology::MatchHost,
            memory_mib: Some(600),
            prefer_staged: true,
            compos_type: CompOsType::OdRefresh,
        };

        let virt_svc_binder = {
            let mock_virt_svc = MockIVirtualizationService::new();
            BnVirtualizationService::new_binder(mock_virt_svc, BinderFeatures::default())
        };
        let system_properties_read_ctx = mock_system_properties::read_context();

        system_properties_read_ctx.expect().with(eq(ART_MEMORY_PROPERTY)).return_once(|_| Ok(None)); // No default override by ART.

        system_properties_read_ctx
            .expect()
            .with(eq(VENDOR_MEMORY_PROPERTY))
            .return_once(|_| Ok(None)); // No memory adjustment.

        let _mock_instance_starter_new_ctx = {
            let mut instance_starter_mock = MockInstanceStarter::default();
            instance_starter_mock.expect_start_new_instance().with(any()).return_once(|_| {
                let vm_instance = MockComposClient::default();
                let mock_compos_service = MockICompOsService::default();
                let mock_compos_service_binder =
                    BnCompOsService::new_binder(mock_compos_service, BinderFeatures::default());
                let service = CompOsService::OdRefresh(mock_compos_service_binder);
                let mut lazy_service_guard = MockLazyServiceGuard::default();
                lazy_service_guard.expect_drop().times(1).return_const(());

                let compos_instance =
                    CompOsInstance::new_for_test(vm_instance, service, lazy_service_guard);
                Ok(compos_instance)
            });
            let ctx = MockInstanceStarter::new_context();
            ctx.expect()
                .with(eq(CURRENT_INSTANCE_DIR), eq(expected_vm_param))
                .return_once(|_, _| instance_starter_mock);
            ctx
        };
        let instance_manager = InstanceManager::new(virt_svc_binder);
        let result = instance_manager.start_current_instance(CompOsType::OdRefresh, BASE_OS);
        assert!(result.is_ok());
    }
}
