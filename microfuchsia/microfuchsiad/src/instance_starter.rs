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

//! Responsible for starting an instance of the Microfuchsia VM.

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    CpuTopology::CpuTopology, IVirtualizationService::IVirtualizationService,
    VirtualMachineConfig::VirtualMachineConfig, VirtualMachineRawConfig::VirtualMachineRawConfig,
};
use anyhow::{ensure, Context, Result};
use binder::{LazyServiceGuard, ParcelFileDescriptor};
use log::info;
use std::ffi::CStr;
use std::fs::File;
use std::os::fd::FromRawFd;
use vmclient::VmInstance;

pub struct MicrofuchsiaInstance {
    _vm_instance: VmInstance,
    _lazy_service_guard: LazyServiceGuard,
    _pty: Pty,
}

pub struct InstanceStarter {
    instance_name: String,
    instance_id: u8,
}

impl InstanceStarter {
    pub fn new(instance_name: &str, instance_id: u8) -> Self {
        Self { instance_name: instance_name.to_owned(), instance_id }
    }

    pub fn start_new_instance(
        &self,
        virtualization_service: &dyn IVirtualizationService,
    ) -> Result<MicrofuchsiaInstance> {
        info!("Creating {} instance", self.instance_name);

        // Always use instance id 0, because we will only ever have one instance.
        let mut instance_id = [0u8; 64];
        instance_id[0] = self.instance_id;

        // Open the kernel and initrd files from the microfuchsia.images apex.
        let kernel_fd =
            File::open("/apex/com.android.microfuchsia.images/etc/linux-arm64-boot-shim.bin")
                .context("Failed to open the boot-shim")?;
        let initrd_fd = File::open("/apex/com.android.microfuchsia.images/etc/fuchsia.zbi")
            .context("Failed to open the fuchsia ZBI")?;
        let kernel = Some(ParcelFileDescriptor::new(kernel_fd));
        let initrd = Some(ParcelFileDescriptor::new(initrd_fd));

        // Prepare a pty for console input/output.
        let pty = openpty()?;
        let console_in = Some(pty.leader.try_clone().context("cloning pty")?);
        let console_out = Some(pty.leader.try_clone().context("cloning pty")?);

        let config = VirtualMachineConfig::RawConfig(VirtualMachineRawConfig {
            name: "Microfuchsia".into(),
            instanceId: instance_id,
            kernel,
            initrd,
            params: None,
            bootloader: None,
            disks: vec![],
            protectedVm: false,
            memoryMib: 256,
            cpuTopology: CpuTopology::ONE_CPU,
            platformVersion: "1.0.0".into(),
            // Fuchsia uses serial for console by default.
            consoleInputDevice: Some("ttyS0".into()),
            ..Default::default()
        });
        let vm_instance = VmInstance::create(
            virtualization_service,
            &config,
            console_out,
            console_in,
            /* log= */ None,
            None,
        )
        .context("Failed to create VM")?;
        vm_instance
            .vm
            .setHostConsoleName(&pty.follower_name)
            .context("Setting host console name")?;
        vm_instance.start().context("Starting VM")?;

        Ok(MicrofuchsiaInstance {
            _vm_instance: vm_instance,
            _lazy_service_guard: Default::default(),
            _pty: pty,
        })
    }
}

struct Pty {
    leader: File,
    follower_name: String,
}

/// Opens a pseudoterminal (pty), configures it to be a raw terminal, and returns the file pair.
fn openpty() -> Result<Pty> {
    // Create a pty pair.
    let mut leader: libc::c_int = -1;
    let mut _follower: libc::c_int = -1;
    let mut follower_name: Vec<libc::c_char> = vec![0; 32];

    // SAFETY: calling openpty with valid+initialized variables is safe.
    // The two null pointers are valid inputs for openpty.
    unsafe {
        ensure!(
            libc::openpty(
                &mut leader,
                &mut _follower,
                follower_name.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0,
            "failed to openpty"
        );
    }

    // SAFETY: calling these libc functions with valid+initialized variables is safe.
    unsafe {
        // Fetch the termios attributes.
        let mut attr = libc::termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0,
            c_lflag: 0,
            c_line: 0,
            c_cc: [0u8; 19],
        };
        ensure!(libc::tcgetattr(leader, &mut attr) == 0, "failed to get termios attributes");

        // Force it to be a raw pty and re-set it.
        libc::cfmakeraw(&mut attr);
        ensure!(
            libc::tcsetattr(leader, libc::TCSANOW, &attr) == 0,
            "failed to set termios attributes"
        );
    }

    // Construct the return value.
    // SAFETY: The file descriptors are valid because openpty returned without error (above).
    let leader = unsafe { File::from_raw_fd(leader) };
    let follower_name: Vec<u8> = follower_name.iter_mut().map(|x| *x as _).collect();
    let follower_name = CStr::from_bytes_until_nul(&follower_name)
        .context("pty filename missing NUL")?
        .to_str()
        .context("pty filename invalid utf8")?
        .to_string();
    Ok(Pty { leader, follower_name })
}
