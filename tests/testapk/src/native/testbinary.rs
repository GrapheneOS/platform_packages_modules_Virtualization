/*
 * Copyright 2024 The Android Open Source Project
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

//! A VM payload that exists to allow testing of the Rust wrapper for the VM payload APIs.

use anyhow::Result;
use com_android_microdroid_testservice::{
    aidl::com::android::microdroid::testservice::{
        IAppCallback::IAppCallback,
        ITestService::{BnTestService, ITestService, PORT},
    },
    binder::{BinderFeatures, ExceptionCode, Interface, Result as BinderResult, Status, Strong},
};
use cstr::cstr;
use log::{error, info};
use std::panic;
use std::process::exit;
use std::string::String;
use std::vec::Vec;

vm_payload::main!(main);

// Entry point of the Service VM client.
fn main() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("microdroid_testlib_rust")
            .with_max_level(log::LevelFilter::Debug),
    );
    // Redirect panic messages to logcat.
    panic::set_hook(Box::new(|panic_info| {
        error!("{panic_info}");
    }));
    if let Err(e) = try_main() {
        error!("failed with {:?}", e);
        exit(1);
    }
}

fn try_main() -> Result<()> {
    info!("Welcome to the Rust test binary");

    vm_payload::run_single_vsock_service(TestService::new_binder(), PORT.try_into()?)
}

struct TestService {}

impl Interface for TestService {}

impl TestService {
    fn new_binder() -> Strong<dyn ITestService> {
        BnTestService::new_binder(TestService {}, BinderFeatures::default())
    }
}

impl ITestService for TestService {
    fn quit(&self) -> BinderResult<()> {
        exit(0)
    }

    fn addInteger(&self, a: i32, b: i32) -> BinderResult<i32> {
        a.checked_add(b).ok_or_else(|| Status::new_exception(ExceptionCode::ILLEGAL_ARGUMENT, None))
    }

    fn getApkContentsPath(&self) -> BinderResult<String> {
        Ok(vm_payload::apk_contents_path().to_string_lossy().to_string())
    }

    fn getEncryptedStoragePath(&self) -> BinderResult<String> {
        Ok(vm_payload::encrypted_storage_path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or("".to_string()))
    }

    fn insecurelyExposeVmInstanceSecret(&self) -> BinderResult<Vec<u8>> {
        let mut secret = vec![0u8; 32];
        vm_payload::get_vm_instance_secret(b"identifier", secret.as_mut_slice());
        Ok(secret)
    }

    // Everything below here is unimplemented. Implementations may be added as needed.

    fn readProperty(&self, _: &str) -> BinderResult<String> {
        unimplemented()
    }
    fn insecurelyExposeAttestationCdi(&self) -> BinderResult<Vec<u8>> {
        unimplemented()
    }
    fn getBcc(&self) -> BinderResult<Vec<u8>> {
        unimplemented()
    }
    fn runEchoReverseServer(&self) -> BinderResult<()> {
        unimplemented()
    }
    fn getEffectiveCapabilities(&self) -> BinderResult<Vec<String>> {
        unimplemented()
    }
    fn getUid(&self) -> BinderResult<i32> {
        unimplemented()
    }
    fn writeToFile(&self, _: &str, _: &str) -> BinderResult<()> {
        unimplemented()
    }
    fn readFromFile(&self, _: &str) -> BinderResult<String> {
        unimplemented()
    }
    fn getFilePermissions(&self, _: &str) -> BinderResult<i32> {
        unimplemented()
    }
    fn getMountFlags(&self, _: &str) -> BinderResult<i32> {
        unimplemented()
    }
    fn requestCallback(&self, _: &Strong<dyn IAppCallback + 'static>) -> BinderResult<()> {
        unimplemented()
    }
    fn readLineFromConsole(&self) -> BinderResult<String> {
        unimplemented()
    }
}

fn unimplemented<T>() -> BinderResult<T> {
    let message = cstr!("Got a call to an unimplemented ITestService method in testbinary.rs");
    error!("{message:?}");
    Err(Status::new_exception(ExceptionCode::UNSUPPORTED_OPERATION, Some(message)))
}
