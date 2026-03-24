/*
 * Copyright (c) 2026, The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! JNI bindings to call into `run_vm` from Java.

use anyhow::{anyhow, Context, Result};
use jni::objects::{JClass, JString};
use jni::sys::jboolean;
use jni::JNIEnv;
use log::{debug, error, info};
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::Mutex;
use vm_launcher::{run_vm, VmConfig};

static RUNNING_VM: Lazy<Mutex<Option<vmclient::VmInstance>>> = Lazy::new(|| Mutex::new(None));

/// Initializes the logger. Called once from Java's static block.
#[no_mangle]
pub extern "system" fn Java_com_android_trusty_vm_benchmarks_TrustyJni_init(
    _env: JNIEnv,
    _class: JClass,
) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("trusty_benchmarks_jni")
            .with_max_level(log::LevelFilter::Debug),
    );
}

/// Boots up trusty vm
#[no_mangle]
pub extern "system" fn Java_com_android_trusty_vm_benchmarks_TrustyJni_bootVm(
    mut env: JNIEnv,
    _class: JClass,
    kernel_path: JString,
    vm_name: JString,
) -> jboolean {
    let vm_name: String = match env.get_string(&vm_name) {
        Ok(value) => value.into(),
        Err(e) => {
            error!("vm_name value not found {e:?}");
            return false.into();
        }
    };

    try_boot_vm(env, kernel_path, vm_name.clone())
        .inspect(|_| info!("{vm_name} VM booted successfully."))
        .inspect_err(|e| error!("Failed to boot {vm_name} VM: {e:?}"))
        .is_ok()
        .into()
}

fn try_boot_vm(mut env: JNIEnv, kernel_path: JString, vm_name: String) -> Result<()> {
    let kernel_path: String = env.get_string(&kernel_path)?.into();

    debug!("Starting {vm_name} VM for benchmark");

    let config = VmConfig {
        kernel: PathBuf::from(kernel_path),
        protected: true,
        name: vm_name,
        memory_size_mib: 16,
        ..Default::default()
    };

    let vm = run_vm(config).context("failed to boot VM")?;
    let mut guard =
        RUNNING_VM.lock().map_err(|e| anyhow!("RUNNING_VM mutex is poisoned {:?}", e))?;
    *guard = Some(vm);
    Ok(())
}

/// Shuts down security_vm
#[no_mangle]
pub extern "system" fn Java_com_android_trusty_vm_benchmarks_TrustyJni_shutdownVm(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    try_shutdown_vm()
        .inspect(|_| info!("Vm shutdown successfully"))
        .inspect_err(|e| error!("failed to shutdown VM : {e:?}"))
        .is_ok()
        .into()
}

fn try_shutdown_vm() -> Result<()> {
    let mut guard =
        RUNNING_VM.lock().map_err(|e| anyhow!("RUNNING_VM mutex is poisoned {:?}", e))?;
    if let Some(vm_instance) = guard.take() {
        vm_instance.stop()?;
    }
    Ok(())
}
