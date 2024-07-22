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

//! Test end-to-end IAccessor implementation with accessor_demo.

use com_android_virt_accessor_demo_vm_service::aidl::com::android::virt::accessor_demo::vm_service::IAccessorVmService::IAccessorVmService;
use binder::{Strong, ProcessState};
use rdroidtest::rdroidtest;

const VM_SERVICE: &str = "com.android.virt.accessor_demo.vm_service.IAccessorVmService/default";

fn init() {
    ProcessState::set_thread_pool_max_thread_count(5);
    ProcessState::start_thread_pool();
}

fn wait_for_interface() -> Strong<dyn IAccessorVmService> {
    binder::wait_for_interface(VM_SERVICE).unwrap()
}

fn get_interface() -> Strong<dyn IAccessorVmService> {
    binder::get_interface(VM_SERVICE).unwrap()
}

fn check_interface() -> Strong<dyn IAccessorVmService> {
    binder::check_interface(VM_SERVICE).unwrap()
}

#[rdroidtest]
fn test_wait_for_interface() {
    init();

    let service = wait_for_interface();
    let sum = service.add(11, 12).unwrap();

    assert_eq!(sum, 23);
}

#[rdroidtest]
fn test_wait_for_interface_twice() {
    init();

    let service1 = wait_for_interface();
    let service2 = wait_for_interface();

    assert_eq!(service1.add(11, 12).unwrap(), 23);
    assert_eq!(service2.add(11, 12).unwrap(), 23);
}

#[rdroidtest]
fn test_wait_and_get_interface() {
    init();

    let service1 = wait_for_interface();
    let service2 = get_interface();

    assert_eq!(service1.add(11, 12).unwrap(), 23);
    assert_eq!(service2.add(11, 12).unwrap(), 23);
}

#[rdroidtest]
fn test_wait_and_check_interface() {
    init();

    let service1 = wait_for_interface();
    let service2 = check_interface();

    assert_eq!(service1.add(11, 12).unwrap(), 23);
    assert_eq!(service2.add(11, 12).unwrap(), 23);
}

rdroidtest::test_main!();
