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

//! Exposes an on-demand binder service to perform system compilation tasks using CompOS. It is
//! responsible for managing the lifecycle of the CompOS VM instances, providing key management for
//! them, and orchestrating trusted compilation.

mod fd_server_helper;
mod instance_manager;
mod instance_starter;
mod odrefresh_task;
mod service;

use crate::instance_manager::InstanceManager;
use anyhow::{Context, Result};
use binder::{register_lazy_service, ProcessState};
use log::{error, info};
use std::panic;
use std::sync::Arc;

#[allow(clippy::eq_op)]
fn try_main() -> Result<()> {
    let debuggable = env!("TARGET_BUILD_VARIANT") != "user";
    let log_level = if debuggable { log::LevelFilter::Debug } else { log::LevelFilter::Info };
    android_logger::init_once(
        android_logger::Config::default().with_tag("composd").with_max_level(log_level),
    );

    // Redirect panic messages to logcat.
    panic::set_hook(Box::new(|panic_info| {
        log::error!("{}", panic_info);
    }));

    ProcessState::start_thread_pool();

    let virtmgr =
        vmclient::VirtualizationService::new().context("Failed to spawn VirtualizationService")?;
    let virtualization_service =
        virtmgr.connect().context("Failed to connect to VirtualizationService")?;

    let instance_manager = Arc::new(InstanceManager::new(virtualization_service));
    let composd_service = service::new_binder(instance_manager);
    register_lazy_service("android.system.composd", composd_service.as_binder())
        .context("Registering composd service")?;

    info!("Registered services, joining threadpool");
    ProcessState::join_thread_pool();

    info!("Exiting");
    Ok(())
}

fn main() {
    if let Err(e) = try_main() {
        error!("{:?}", e);
        std::process::exit(1)
    }
}
