// Copyright 2021, The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Functions for running instances of `crosvm`.

use crate::aidl;
use crate::atom::{get_num_cpus, write_vm_exited_stats_sync};
use crate::composite;
use crate::debug_config::DebugConfig;
use crate::virtualmachine::{self, Cid, VirtualMachineCallbacks};
use anyhow::{anyhow, bail, Context, Error, Result};
use avflog::LogResult;
use binder::{DeathRecipient, ParcelFileDescriptor, Strong};
use command_fds::CommandFdExt;
use libc::{sysconf, _SC_CLK_TCK};
use log::{debug, error, info, warn};
use nix::{
    errno::Errno,
    fcntl::OFlag,
    sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags, EpollTimeout},
    sys::eventfd::EventFd,
    sys::wait::{waitid, Id, WaitPidFlag, WaitStatus},
    unistd::{pipe2, Pid, Uid, User},
};
use psi_rs::{init_psi_monitor, parse_psi_line, register_psi_monitor, PsiResource, PsiStallType};
use regex::{Captures, Regex};
use rpcbinder::RpcServer;
use rustutils::android::system_properties;
use semver::{Version, VersionReq};
use shared_child::SharedChild;
use std::borrow::Cow;
use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::ffi::{CString, OsStr, OsString};
use std::fmt;
use std::fs::{canonicalize, read_to_string, File, OpenOptions};
use std::io::{self, Read, Seek};
use std::mem;
use std::num::{NonZeroU16, NonZeroU32};
use std::os::unix::io::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use std::time::{Duration, SystemTime};
use tombstoned_client::{DebuggerdDumpType, TombstonedConnection};

const CROSVM_PATH: &str = "/apex/com.android.virt/bin/crosvm";

/// Version of the platform that crosvm currently implements. The format follows SemVer. This
/// should be updated when there is a platform change in the crosvm side. Having this value here is
/// fine because virtualizationservice and crosvm are supposed to be updated together in the virt
/// APEX.
const CROSVM_PLATFORM_VERSION: &str = "1.0.0";

/// The exit status which crosvm returns when it has an error starting a VM.
const CROSVM_START_ERROR_STATUS: i32 = 1;
/// The exit status which crosvm returns when a VM requests a reboot.
const CROSVM_REBOOT_STATUS: i32 = 32;
/// The exit status which crosvm returns when it crashes due to an error.
const CROSVM_CRASH_STATUS: i32 = 33;
/// The exit status which crosvm returns when vcpu is stalled.
const CROSVM_WATCHDOG_REBOOT_STATUS: i32 = 36;

const MILLIS_PER_SEC: i64 = 1000;

const SYSPROP_CUSTOM_PVMFW_PATH: &str = "hypervisor.pvmfw.path";

/// Serial device for VM console input.
/// Hypervisor (virtio-console)
const CONSOLE_HVC0: &str = "hvc0";
/// Serial (emulated uart)
const CONSOLE_TTYS0: &str = "ttyS0";

/// virtio-console input usage is uncommon in AVF and it consumes a lot of memory (one page per
/// entry), so make the RX as small as possible.
/// The `virtio_drivers` crate requires a size of at least 2.
const CONSOLE_RX_QUEUE_SIZE: u32 = 2;
const CONSOLE_TX_QUEUE_SIZE: u32 = 32;

/// If the VM doesn't move to the Started state within this amount time, a hang-up error is
/// triggered.
static BOOT_HANGUP_TIMEOUT: LazyLock<Duration> = LazyLock::new(|| {
    if nested_virt::is_nested_virtualization().unwrap() {
        // Nested virtualization is slow, so we need a longer timeout.
        Duration::from_secs(300)
    } else {
        Duration::from_secs(60)
    }
});

/// Configuration for a VM to run with crosvm.
pub struct CrosvmConfig {
    pub cid: Cid,
    pub name: String,
    pub shared_paths: Vec<SharedPathConfig>,
    pub protected: bool,
    pub detect_hangup: bool,
    pub device_tree_overlays: Vec<File>,
    pub start_suspended: bool,
    pub command: CrosvmCommand,
}

fn try_into_non_zero_u32(value: i32) -> Result<NonZeroU32> {
    let u32_value = value.try_into()?;
    NonZeroU32::new(u32_value).ok_or(anyhow!("value should be greater than 0"))
}

/// Shared path between host and guest VM.
#[derive(Debug)]
pub struct SharedPathConfig {
    pub path: String,
    pub host_uid: i32,
    pub host_gid: i32,
    pub guest_uid: i32,
    pub guest_gid: i32,
    pub mask: i32,
    pub tag: String,
    pub socket_path: String,
    pub socket_fd: Option<File>,
    pub app_domain: bool,
}

/// All information needed for running crosvm
pub struct RunContext<'a> {
    pub config: &'a aidl::VirtualMachineRawConfig,
    pub debug_config: &'a DebugConfig,
    pub cid: Cid,
    pub temp_dir: &'a Path,
    pub console_out: Option<&'a ParcelFileDescriptor>,
    pub console_in: Option<&'a ParcelFileDescriptor>,
    pub log_out: Option<&'a ParcelFileDescriptor>,
    pub devicetree_dump_out: Option<&'a ParcelFileDescriptor>,
}

/// Parses RunContext into raw arguments which will be used to construct a crosvm command. The
/// parsing is done when the virtual machine is created, and the construction of the crosvm command
/// is done when the virtual machine is started.
pub struct CrosvmCommand {
    arg0: OsString,
    args: Vec<OsString>,
    preserved_fds: Vec<OwnedFd>,
    // List of lambdas which need to run after crosvm exits. Option is added since this will be
    // moved out of this struct when the VM gets run. Box is needed to satisfy the fixed-size
    // requirement of Vec.
    cleaners: Option<HashMap<String, Box<Cleaner>>>,
    balloon_enabled: bool,
}

type Cleaner = dyn FnOnce(&CleanerContext) -> Result<()> + Send;

struct CleanerContext {
    failure_reason: Mutex<String>,
}

impl CrosvmCommand {
    pub fn build_from(context: &RunContext) -> Result<Self> {
        Self::check_platform_version(context)?;

        let mut command = Self {
            arg0: OsString::new(),
            args: Vec::new(),
            preserved_fds: Vec::new(),
            cleaners: Some(HashMap::new()),
            balloon_enabled: false,
        };
        command
            .arg("--extended-status")
            // Logs are further filtered in logcat per process, debug logs won't show unless
            // crosvm is configured to show debug logs.
            .args(["--log-level", "debug,disk=warn"])
            .arg("run")
            .arg("--disable-sandbox") // TODO(qwandor): Remove --disable-sandbox.
            .args(["--cid", &context.cid.to_string()]);

        command.add_name_arg(context);
        command.add_pvm_arg(context)?;
        command.add_kernel_arg(context)?;
        command.add_cpu_arg(context)?;
        #[cfg(target_arch = "aarch64")]
        command.add_aarch64_specific_args(context);
        command.add_memory_arg(context);
        command.add_balloon_arg(context);
        command.add_console_arg(context)?;
        command.add_log_arg(context)?;
        command.add_failure_pipe()?;
        command.add_ramdump_arg(context)?;
        command.add_disk_arg(context)?;
        command.add_gpu_arg(context)?;
        command.add_input_devices_arg(context)?;
        command.add_audio_arg(context);
        command.add_usb_arg(context);
        command.add_network_arg(context)?;
        command.add_file_backed_mapping_arg(context)?;
        command.add_assigned_devices_arg(context)?;
        command.add_dump_dtb_arg(context)?;
        command.add_gdb_arg(context)?;
        command.add_gunyah_specific_arg(context)?;
        command.add_teeservices_arg(context)?;
        Ok(command)
    }

    fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().into());
        self
    }

    fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(&mut self, args: I) -> &mut Self {
        for arg in args {
            self.arg(arg.as_ref());
        }
        self
    }

    #[allow(unused)]
    fn add_preserved_fd<F: Into<OwnedFd>>(&mut self, file: F) -> String {
        let fd = file.into();
        let raw_fd = fd.as_raw_fd();
        self.preserved_fds.push(fd);
        format!("/proc/self/fd/{raw_fd}")
    }

    fn add_cleaner(&mut self, name: &str, cleaner: Box<Cleaner>) -> Result<()> {
        if self.cleaners.as_mut().unwrap().insert(name.to_owned(), cleaner).is_some() {
            Err(anyhow!("cleaner with name {name} already exists."))
        } else {
            Ok(())
        }
    }

    fn check_platform_version(context: &RunContext) -> Result<()> {
        let ver = &context.config.platformVersion;
        let requested = VersionReq::parse(ver)
            .context(format!("Invalid platform version requirement {ver}"))?;

        let supported = Version::parse(CROSVM_PLATFORM_VERSION).unwrap();
        if !requested.matches(&supported) {
            bail!(
                "Incompatible platform version. The config is compatible with platform version(s) \
                  {}, but the actual platform version is {}",
                requested,
                supported
            );
        }
        Ok(())
    }

    fn add_name_arg(&mut self, context: &RunContext) {
        let name = "crosvm_".to_owned() + &context.config.name;
        self.arg0 = OsString::from(name.clone());
        self.args(["--name", &name]);
    }

    fn add_pvm_arg(&mut self, context: &RunContext) -> Result<()> {
        if !context.config.protectedVm {
            if let Some(custom_pvmfw) = &context.config.customPvmfw {
                let custom_pvmfw_fd = self.add_preserved_fd(custom_pvmfw.as_ref().try_clone()?);
                self.arg("--unprotected-vm-with-firmware").arg(&custom_pvmfw_fd);
            }
            return Ok(());
        }

        match system_properties::read(SYSPROP_CUSTOM_PVMFW_PATH)? {
            Some(pvmfw_path) if !pvmfw_path.is_empty() => {
                if pvmfw_path == "none" {
                    self.arg("--protected-vm-without-firmware")
                } else {
                    self.args(["--protected-vm-with-firmware", &pvmfw_path])
                }
            }
            _ => {
                if let Some(custom_pvmfw) = &context.config.customPvmfw {
                    let custom_pvmfw_fd = self.add_preserved_fd(custom_pvmfw.as_ref().try_clone()?);
                    self.arg("--protected-vm-with-firmware").arg(&custom_pvmfw_fd)
                } else {
                    self.arg("--protected-vm")
                }
            }
        };

        // Workaround to keep crash_dump from trying to read protected guest memory.
        // Context in b/238324526.
        self.arg("--unmap-guest-memory-on-fork");

        // Lock the guest memory to improve memory accounting. More context in b/407786138
        self.arg("--lock-guest-memory");

        Ok(())
    }

    fn add_kernel_arg(&mut self, context: &RunContext) -> Result<()> {
        let config = context.config;
        if config.bootloader.is_none() && config.kernel.is_none() {
            bail!("VM must have either a bootloader or a kernel image.");
        }
        if config.bootloader.is_some() && (config.kernel.is_some() || config.initrd.is_some()) {
            bail!("Can't have both bootloader and kernel/initrd image.");
        }

        if let Some(bootloader) = &config.bootloader {
            let file = self.add_preserved_fd(bootloader.as_ref().try_clone()?);
            self.args(["--bios", &file]);
        }

        if let Some(kernel) = &config.kernel {
            let file = self.add_preserved_fd(kernel.as_ref().try_clone()?);
            self.arg(file);
        }

        if let Some(params) = &config.params {
            self.args(["--params", params]);
        }

        if !config.name.is_empty() {
            if !is_valid_vm_name(&config.name) {
                bail!("Invalid VM name \"{}\"", config.name);
            }
            let hostname_param = format!("hostname={}", config.name);
            self.args(["--params", &hostname_param]);
        }

        if let Some(initrd) = &config.initrd {
            let file = self.add_preserved_fd(initrd.as_ref().try_clone()?);
            self.args(["--initrd", &file]);
        }
        Ok(())
    }

    fn add_cpu_arg(&mut self, context: &RunContext) -> Result<()> {
        let config = context.config;
        let num_cores: Option<usize> = match &config.cpuOptions.cpuTopology {
            aidl::CpuTopology::MatchHost(_) => {
                if check_if_all_cpus_allowed()? {
                    None
                } else {
                    Some(get_num_cpus().context("can't get number of CPUs")?)
                }
            }
            aidl::CpuTopology::CpuCount(count) => Some((*count).try_into().unwrap()),
        };

        let mut cpu_args = Vec::new();
        if let Some(num_cores) = num_cores {
            cpu_args.push(format!("num-cores={num_cores}"));
        } else {
            self.arg("--host-cpu-topology");
            #[cfg(target_arch = "aarch64")]
            {
                if cfg!(virt_cpufreq_upstream) {
                    self.arg("--virt-cpufreq-upstream");
                } else {
                    self.arg("--virt-cpufreq");
                }
            }
        }

        #[cfg(target_arch = "aarch64")]
        cpu_args.push("sve=[auto=true]".to_string());

        if !cpu_args.is_empty() {
            self.args(["--cpus", &cpu_args.join(",")]);
        }

        if context.config.boostUclamp {
            self.arg("--boost-uclamp");
        }

        Ok(())
    }

    fn add_gunyah_specific_arg(&mut self, context: &RunContext) -> Result<()> {
        if !hypervisor_props::is_gunyah()? {
            if context.config.enableHypervisorSpecificAuthMethod {
                bail!("hypervisor specific auth method not supported for current hypervisor");
            }
            return Ok(());
        }

        if context.config.enableHypervisorSpecificAuthMethod {
            if !context.config.protectedVm {
                bail!("hypervisor specific auth method only supported for protected VMs");
            }

            // "QCOM Trusted VM" compatibility mode.
            //
            // When this mode is enabled, two hypervisor specific IDs are expected to be packed
            // into the instance ID. We extract them here and pass along to crosvm so they can be
            // given to the hypervisor driver via an ioctl.
            let pas_id = u32::from_le_bytes(context.config.instanceId[60..64].try_into().unwrap());
            let vm_id = u16::from_le_bytes(context.config.instanceId[58..60].try_into().unwrap());
            self.args(["--hypervisor",
                &format!("gunyah[device=/dev/gunyah,qcom_trusted_vm_id={vm_id},qcom_trusted_vm_pas_id={pas_id}]")]);
            // Put the FDT close to the payload (default is end of RAM) to so that CMA can be used
            // without bloating memory usage.
            self.args(["--fdt-position", "after-payload"]);
        }
        Ok(())
    }

    #[cfg(target_arch = "aarch64")]
    fn add_aarch64_specific_args(&mut self, context: &RunContext) {
        // Move the PCI MMIO regions to near the end of the low-MMIO space.
        // This is done to accommodate a limitation in a partner's hypervisor.
        self.args([
            "--pci",
            "mem=[start=0x2c000000,size=0x2000000],cam=[start=0x2e000000,size=0x1000000]",
        ]);
        self.arg("--no-pmu");

        let allow_vgic_its = !context.config.protectedVm || context.config.allowVgicItsInPvm;
        self.arg(format!("--irqchip=kernel[allow-vgic-its={allow_vgic_its}]"));
    }

    fn add_memory_arg(&mut self, context: &RunContext) {
        let config = context.config;
        let mut memory_mib = config
            .memoryMib
            .try_into()
            .ok()
            .and_then(NonZeroU32::new)
            .unwrap_or(NonZeroU32::new(256).unwrap());

        let swiotlb_size_mib = Self::get_swiotlb_mib(config);

        // b/346770542 for consistent "usable" memory across protected and non-protected VMs.
        memory_mib = memory_mib.saturating_add(swiotlb_size_mib);
        // For consistent "usable" memory across debuggable and non-debuggable VMs.
        memory_mib = memory_mib.saturating_add(Self::get_ramdump_mib(context));
        self.args(["--mem", &memory_mib.get().to_string()]);

        // b/316956218: Some pKVM versions incorrectly advertise KVM_CAP_READONLY_MEM, even for
        // non-protected VMs, so force it off.
        self.arg("--force-disable-readonly-mem");

        // TODO(b/462545596): Enable on x86 once crosvm supports it. It is inentional that we
        // continue to pad the RAM size with `swiotlb_size_mib`.
        #[cfg(target_arch = "aarch64")]
        if swiotlb_size_mib > 0 {
            self.args(["--swiotlb", &swiotlb_size_mib.to_string()]);
        }

        if context.config.hugePages {
            self.arg("--hugepages");
        }

        // b/432329937#comment3; At the moment, microdroid kernels do not interface with other
        // peripherals/DMA engines, so there should not be a need for coherent DMA pools.
        // We can't completely eliminate them, so reduce them to be as small as possible.
        //
        // Without this, the kernel will default to allocating 128 KB of memory for each GB
        // provisioned to a VM, which could result in several pools being created depending
        // on the IPA space layout of the VM.
        //
        // For instance, on a VM that has memory in both ZONE_DMA and ZONE_NORMAL, two pools
        // will be allocated to service allocations that target each zone. On a small VM (less than
        // 1 GB in size), that results in 256 KB being used. The same thing can also happen on a VM
        // that has all of memory in ZONE_DMA and is under investigation as well in b/432329937.
        //
        // TODO: This should really be the guest's page size.
        self.args(["--params", "coherent_pool=4096"]);

        // b/437196556. The kernel maintains a linear mapping of all of RAM (i.e. virtually and
        // physically contiguous mapping). Any pages that aren't kernel code are mapped as
        // read/write.
        //
        // The kernel can be extended via loadable kernel modules or BPF programs, which have
        // read-only sections that need to be resident in memory. When loading these sections,
        // the kernel allocates virtually contiguous but not physically contiguous memory, and
        // the virtual address range is mapped as read-only. However, the linear mapping to those
        // physical pages still remains as writable, which is not ideal.
        //
        // The rodata feature--when set to "full"--allows the linear mapping to be updated to
        // read-only to match the virtual mapping to eliminate the possibility of the data
        // being overwritten. However, this comes at the cost of having to map RAM down to the base
        // page size, instead of using block mappings, which results in additional memory usage for
        // pagetables. Additionally, the number of pagetable entries can cause additional TLB
        // pressure, and degrade performance.
        //
        // Using rodata=on makes it so that the kernel image's read-only sections cannot be written
        // to, and does not require mapping RAM down to the base page size, so lets use that to
        // save some memory.
        //
        // TODO: Remove this after the default conditions for enabling
        // CONFIG_RODATA_FULL_DEFAULT_ENABLED have been modified in the kernel so that it is only
        // set when it may be needed (i.e. if the kernel supports loadable kernel modules or BPF
        // programs.).
        if virtualmachine::check_use_relaxed_microdroid_rollback_protection().is_ok() {
            self.args(["--params", "rodata=on"]);
        }
    }

    fn add_balloon_arg(&mut self, context: &RunContext) {
        let supported = system_properties::read_bool("hypervisor.memory_reclaim.supported", false)
            .unwrap_or(false);
        let requested = context.config.balloon;
        let enabled = requested && supported;

        if enabled {
            self.balloon_enabled = true;
            self.arg("--balloon-page-reporting");
        } else {
            warn!(
                "Memory balloon not enabled:
                config.balloon={requested},hypervisor.memory_reclaim.supported={supported}"
            );
            self.arg("--no-balloon");
        }
    }

    // A note on serial devices. We have five serial devices:
    // 1. uart device: used as the output device by bootloaders and as early console by linux
    // 2. uart device: used to report the reason for the VM failing.
    // 3. virtio-console device: used as the console device where kmsg is redirected to
    // 4. virtio-console device: used as the ramdump output
    // 5. virtio-console device: used as the logcat output
    //
    // #1 and #3 are added via add_console_arg()
    // #2 is added via add_failure_pipe()
    // #4 is added via add_ramdump_arg()
    // #5 is added via add_log_arg()
    //
    // When [console|log]_fd is not specified, the devices are attached to sink, which means what's
    // written there is discarded.
    //
    // Warning: Adding more serial devices requires you to shift the PCI device ID of the boot
    // disks in bootconfig.x86_64. This is because x86 crosvm puts serial devices and the block
    // devices in the same PCI bus and serial devices comes before the block devices. Arm crosvm
    // doesn't have the issue.
    fn add_console_arg(&mut self, context: &RunContext) -> Result<()> {
        // If user has provided an FD for console_out, let them read from it. Otherwise, we read
        // the console output from the VM and emit it over to logcat.
        let (out_fd, read_file) = match context.console_out {
            Some(pfd) => (Some(pfd.as_ref().try_clone()?), None),
            None => {
                let (read_fd, write_fd) = create_pipe()?;
                (Some(write_fd.into()), Some(read_fd))
            }
        };

        let in_fd = context.console_in.map(|pfd| pfd.as_ref().try_clone()).transpose()?;

        let in_device = context.config.consoleInputDevice.as_deref().unwrap_or(CONSOLE_HVC0);
        match in_device {
            CONSOLE_HVC0 | CONSOLE_TTYS0 => {}
            _ => bail!("Unsupported serial device {in_device}"),
        };

        if context.debug_config.debug_level == aidl::DebugLevel::NONE
            && context.debug_config.should_prepare_console_output()
        {
            // bootconfig.normal will be used, but we need log.
            self.args(["--params", "printk.devkmsg=on"]);
            self.args(["--params", "console=hvc0"]);
        }

        let out_args = out_fd.map_or("type=sink".to_string(), |fd| {
            format!("type=file,path={}", self.add_preserved_fd(fd))
        });

        let in_args =
            in_fd.map_or("".to_string(), |fd| format!(",input={}", self.add_preserved_fd(fd)));

        // dev/ttyS0
        self.arg(format!(
            "--serial={out_args}{},hardware=serial,num=1",
            if in_device == CONSOLE_TTYS0 { &in_args } else { "" }
        ));
        // dev/hvc0
        self.arg(format!(
            "--serial={out_args}{},hardware=virtio-console,num=1,\
                    max-queue-sizes=[{CONSOLE_RX_QUEUE_SIZE},{CONSOLE_TX_QUEUE_SIZE}]",
            if in_device == CONSOLE_HVC0 { &in_args } else { "" }
        ));

        let thread = read_file.map(|f| Self::logger_thread(f, format!("Console({})", context.cid)));
        let cleaner = move |_: &CleanerContext| {
            thread.map(JoinHandle::join);
            Ok(())
        };
        self.add_cleaner("console", Box::new(cleaner))?;
        Ok(())
    }

    fn add_log_arg(&mut self, context: &RunContext) -> Result<()> {
        let (out_fd, read_file) = match context.log_out {
            Some(pfd) => (Some(pfd.as_ref().try_clone()?), None),
            None => {
                let (read_fd, write_fd) = create_pipe()?;
                (Some(write_fd.into()), Some(read_fd))
            }
        };

        let out_args = out_fd.map_or("type=sink".to_string(), |fd| {
            format!("type=file,path={}", self.add_preserved_fd(fd))
        });

        // dev/hvc2
        self.arg(format!(
            "--serial={out_args},hardware=virtio-console,num=3,\
                    max-queue-sizes=[{CONSOLE_RX_QUEUE_SIZE},{CONSOLE_TX_QUEUE_SIZE}]"
        ));

        let thread = read_file.map(|f| Self::logger_thread(f, format!("Log({})", context.cid)));
        let cleaner = move |_: &CleanerContext| {
            thread.map(JoinHandle::join);
            Ok(())
        };
        self.add_cleaner("log", Box::new(cleaner))?;
        Ok(())
    }

    fn logger_thread(read_from: File, tag: String) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("logger".to_string())
            .spawn(move || {
                use std::io::BufRead;
                let mut reader = std::io::BufReader::new(read_from);
                let mut buf = vec![];
                loop {
                    buf.clear();
                    buf.shrink_to(1024);
                    match reader.read_until(b'\n', &mut buf) {
                        Ok(0) => {
                            info!("{}: EOF", &tag);
                            return;
                        }
                        Ok(_size) => {
                            if buf.last() == Some(&b'\n') {
                                buf.pop();
                                // Logs sent via TTY usually end lines with "\r\n".
                                if buf.last() == Some(&b'\r') {
                                    buf.pop();
                                }
                            }
                            info!("{}: {}", &tag, &String::from_utf8_lossy(&buf));
                        }
                        Err(e) => {
                            error!("Could not read console pipe: {e:?}");
                            return;
                        }
                    };
                }
            })
            .expect("Failed to create logger thread")
    }

    fn add_failure_pipe(&mut self) -> Result<()> {
        let (reader, writer) = create_pipe()?;
        let writer = self.add_preserved_fd(writer);
        // This becomes /dev/ttyS1
        self.arg(format!("--serial=type=file,path={writer},hardware=serial,num=2"));

        let read_thread = std::thread::Builder::new()
            .name("failure_reader".to_string())
            .spawn(move || {
                // Read the pipe to see if any failure reason is written
                let mut failure_reason = String::new();
                // Arbitrary max size in case of misbehaving guest.
                const MAX_SIZE: u64 = 50_000;
                match reader.take(MAX_SIZE).read_to_string(&mut failure_reason) {
                    Err(e) => error!("Error reading VM failure reason from pipe: {e}"),
                    Ok(len) if len > 0 => {
                        error!("VM returned failure reason '{}'", failure_reason.trim())
                    }
                    _ => (),
                };
                failure_reason.trim().to_owned()
            })
            .expect("Failed to create failure_reader thread");

        let cleaner = move |context: &CleanerContext| {
            let failure_reason = read_thread.join().expect("Failed to wait for fail reason");

            *context.failure_reason.lock().unwrap() = failure_reason;
            Ok(())
        };
        self.add_cleaner("failure_pipe", Box::new(cleaner))?;
        Ok(())
    }

    /// The size of memory (in MiB) reserved for ramdump.
    fn get_ramdump_mib(context: &RunContext) -> u32 {
        let config = context.config;
        let using_gki =
            if !cfg!(vendor_module) { false } else { config.osName.starts_with("microdroid_gki-") };
        if context.debug_config.is_ramdump_needed() && !using_gki {
            17 + Self::get_swiotlb_mib(config)
        } else {
            0
        }
    }

    fn get_swiotlb_mib(config: &aidl::VirtualMachineRawConfig) -> u32 {
        if !config.protectedVm {
            0
        } else if config.swiotlbMib > 0 {
            config.swiotlbMib.try_into().unwrap()
        } else {
            estimate_swiotlb_usage_mib(SwiotlbEstimateInputs {
                guest_page_size: 4096, // TODO: Use real page size.
                block_count: config.disks.len().try_into().unwrap(),
                console_count: 3,
                balloon: config.balloon,
            })
        }
    }

    fn add_ramdump_arg(&mut self, context: &RunContext) -> Result<()> {
        let reserve = Self::get_ramdump_mib(context);
        if reserve > 0 {
            // `ramdump_write` is sent to crosvm and will be the backing store for the /dev/hvc1
            // where VM will emit ramdump to. `ramdump_read` will be sent back to the client (i.e.
            // the VM owner) for readout.
            let file = File::create(context.temp_dir.join("ramdump"))?;
            let path = self.add_preserved_fd(file);

            // This becoms /dev/hvc1 (see num=2 below)
            self.arg(format!(
                "--serial=type=file,path={path},hardware=virtio-console,num=2,\
                    max-queue-sizes=[{CONSOLE_RX_QUEUE_SIZE},{CONSOLE_TX_QUEUE_SIZE}]"
            ));

            self.args(["--params", &format!("crashkernel={reserve}M")]);
        } else {
            self.arg(format!(
                "--serial=type=sink,hardware=virtio-console,num=2,\
                    max-queue-sizes=[{CONSOLE_RX_QUEUE_SIZE},{CONSOLE_TX_QUEUE_SIZE}]"
            ));
        }
        Ok(())
    }

    fn add_disk_arg(&mut self, context: &RunContext) -> Result<()> {
        /// The size of zero.img.
        /// Gaps in composite disk images are filled with a shared zero.img.
        const ZERO_FILLER_SIZE: u64 = 4096;
        let temp_dir = context.temp_dir;

        let zero_filler = temp_dir.join("zero.img");
        OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&zero_filler)
            .context(format!("Failed to create {zero_filler:?}"))?
            .set_len(ZERO_FILLER_SIZE)?;

        for (index, disk) in context.config.disks.iter().enumerate() {
            let image = if !disk.partitions.is_empty() {
                if disk.image.is_some() {
                    bail!("DiskImage {:?} contains both image and partitions.", disk);
                }

                let composite = temp_dir.join(format!("composite-{index}.img"));
                let header = temp_dir.join(format!("composite-{index}-header.img"));
                let footer = temp_dir.join(format!("composite-{index}-footer.img"));

                let (image, partition_files) = composite::make_composite_image(
                    &disk.partitions,
                    &zero_filler,
                    &composite,
                    &header,
                    &footer,
                )
                .with_context(|| {
                    format!("Failed to make composite disk image with config {disk:?}")
                })
                .with_log()?;

                // These partition files are not directly shown in the command line, but
                // indirectly via the composite disk file. So we need to preserve their FDs.
                partition_files.into_iter().for_each(|f| {
                    self.add_preserved_fd(f);
                });

                image
            } else if let Some(image) = &disk.image {
                image.as_ref().try_clone()?.into()
            } else {
                bail!("DiskImage {:?} didn't contain image or partitions.", disk);
            };

            let path = self.add_preserved_fd(image);
            self.args(["--block", &format!("path={},ro={},lock=false", path, !disk.writable)]);
        }
        Ok(())
    }

    fn add_gpu_arg(&mut self, context: &RunContext) -> Result<()> {
        let config = context.config;
        let mut gpu_args = Vec::new();
        if let Some(config) = &config.gpuConfig {
            if !cfg!(paravirtualized_devices) {
                warn!("GPU configuration not supported. Ignoring");
                return Ok(());
            }
            if let Some(b) = &config.backend {
                gpu_args.push(format!("backend={b}"));
            }
            if let Some(t) = &config.contextTypes {
                // flatten is to convert Vec<Option<String>> into Vec<String>
                let t: Vec<_> = t.clone().into_iter().flatten().collect();
                gpu_args.push(format!("context-types={}", t.join(":")));
            }
            if let Some(a) = &config.pciAddress {
                gpu_args.push(format!("pci-address={a}"));
            }
            if let Some(f) = &config.rendererFeatures {
                gpu_args.push(format!("renderer-features={f}"));
            }
            if config.rendererUseEgl {
                gpu_args.push("egl=true".to_string());
            }
            if config.rendererUseGles {
                gpu_args.push("gles=true".to_string());
            }
            if config.rendererUseGlx {
                gpu_args.push("glx=true".to_string());
            }
            if config.rendererUseSurfaceless {
                gpu_args.push("surfaceless=true".to_string());
            }
            if config.rendererUseVulkan {
                gpu_args.push("vulkan=true".to_string());
            }
        }
        let name = &config.name;
        if let Some(config) = &config.displayConfig {
            if !cfg!(paravirtualized_devices) {
                warn!("Display configuration not supported. Ignoring");
                return Ok(());
            }
            gpu_args.push(format!(
                "displays=[[mode=windowed[{},{}],dpi=[{},{}],refresh-rate={}]]",
                try_into_non_zero_u32(config.width)?,
                try_into_non_zero_u32(config.height)?,
                try_into_non_zero_u32(config.horizontalDpi)?,
                try_into_non_zero_u32(config.verticalDpi)?,
                try_into_non_zero_u32(config.refreshRate)?,
            ));
            self.arg(format!("--android-display-service={name}"));
        }
        if !gpu_args.is_empty() {
            self.arg(format!("--gpu={}", gpu_args.join(",")));
        }
        Ok(())
    }

    fn add_input_devices_arg(&mut self, context: &RunContext) -> Result<()> {
        let config = context.config;
        if !cfg!(paravirtualized_devices) && !config.inputDevices.is_empty() {
            warn!("Input device configuration not supported. Ignoring");
            return Ok(());
        }
        for dev in &config.inputDevices {
            self.arg("--input");
            match dev {
                aidl::InputDevice::SingleTouch(dev) => {
                    let mut params = Vec::new();
                    let pfd = dev.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?;
                    let file = self.add_preserved_fd(pfd.as_ref().try_clone()?);
                    params.push(format!("path={file}"));
                    params.push(format!("width={}", u32::try_from(dev.width)?));
                    params.push(format!("height={}", u32::try_from(dev.height)?));
                    if !dev.name.is_empty() {
                        params.push(format!("name={}", dev.name));
                    }
                    self.arg(format!("single-touch[{}]", params.join(",")));
                }
                aidl::InputDevice::MultiTouch(dev) => {
                    let mut params = Vec::new();
                    let pfd = dev.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?;
                    let file = self.add_preserved_fd(pfd.as_ref().try_clone()?);
                    params.push(format!("path={file}"));
                    params.push(format!("width={}", u32::try_from(dev.width)?));
                    params.push(format!("height={}", u32::try_from(dev.height)?));
                    if !dev.name.is_empty() {
                        params.push(format!("name={}", dev.name));
                    }
                    self.arg(format!("multi-touch[{}]", params.join(",")));
                }
                aidl::InputDevice::Trackpad(dev) => {
                    let mut params = Vec::new();
                    let pfd = dev.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?;
                    let file = self.add_preserved_fd(pfd.as_ref().try_clone()?);
                    params.push(format!("path={file}"));
                    params.push(format!("width={}", u32::try_from(dev.width)?));
                    params.push(format!("height={}", u32::try_from(dev.height)?));
                    if !dev.name.is_empty() {
                        params.push(format!("name={}", dev.name));
                    }
                    self.arg(format!("multi-touch-trackpad[{}]", params.join(",")));
                }
                aidl::InputDevice::EvDev(dev) => {
                    let pfd = dev.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?;
                    let file = self.add_preserved_fd(pfd.as_ref().try_clone()?);
                    self.arg(format!("evdev[path={file}]"));
                }
                aidl::InputDevice::Keyboard(dev) => {
                    let pfd = dev.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?;
                    let file = self.add_preserved_fd(pfd.as_ref().try_clone()?);
                    self.arg(format!("keyboard[path={file}]"));
                }
                aidl::InputDevice::Mouse(dev) => {
                    let pfd = dev.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?;
                    let file = self.add_preserved_fd(pfd.as_ref().try_clone()?);
                    self.arg(format!("mouse[path={file}]"));
                }
                aidl::InputDevice::Switches(dev) => {
                    let pfd = dev.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?;
                    let file = self.add_preserved_fd(pfd.as_ref().try_clone()?);
                    self.arg(format!("switches[path={file}]"));
                }
            }
        }
        Ok(())
    }

    fn add_audio_arg(&mut self, context: &RunContext) {
        let config = context.config;
        if let Some(config) = &config.audioConfig {
            if !cfg!(paravirtualized_devices) {
                warn!("Audio configuration not supported. Ignoring");
                return;
            }
            self.arg("--virtio-snd");
            self.arg(format!(
                "backend=aaudio,num_input_devices={},num_output_devices={}",
                if config.useMicrophone { 1 } else { 0 },
                if config.useSpeaker { 1 } else { 0 },
            ));
        }
    }

    fn add_usb_arg(&mut self, context: &RunContext) {
        let config = context.config;
        let use_usb = if let Some(config) = &config.usbConfig { config.controller } else { false };
        if !use_usb {
            self.arg("--no-usb");
        }
    }

    fn add_network_arg(&mut self, context: &RunContext) -> Result<()> {
        let config = context.config;
        if config.networkSupported {
            if !cfg!(network) {
                warn!("Networking not supported. Ignoring");
                return Ok(());
            }

            if config.protectedVm {
                bail!("Network feature is not supported for pVM yet");
            }

            if cfg!(early) {
                bail!("Network feature is not supported for early VMs yet");
            }

            let tap_fd = {
                let iface_suffix = std::process::id().to_string();
                let pfd = virtualmachine::global_service()
                    .unwrap()
                    .createTapInterface(&iface_suffix)
                    .context(format!(
                        "Failed to create a TAP interface with suffix {iface_suffix}"
                    ))?;
                pfd.as_ref().try_clone()?
            };
            let tap_fd_cloned = tap_fd.try_clone()?;

            let path = self.add_preserved_fd(tap_fd);
            let fd_num = path.split('/').next_back().unwrap();
            self.args(["--net", &format!("tap-fd={fd_num}")]);

            let cleaner = move |_: &CleanerContext| {
                let pfd = ParcelFileDescriptor::new(tap_fd_cloned);
                virtualmachine::global_service()
                    .unwrap()
                    .deleteTapInterface(&pfd)
                    .context("Error deleting TAP interface")?;
                Ok(())
            };
            self.add_cleaner("network", Box::new(cleaner))?;
        }
        Ok(())
    }

    fn add_file_backed_mapping_arg(&mut self, context: &RunContext) -> Result<()> {
        for bf in &context.config.customMemoryBackingFiles {
            let pfd = bf.file.as_ref().ok_or(anyhow!("missing CustomMemoryBackingFile FD"))?;
            let mem_fd = pfd.as_ref().try_clone()?;
            let path = self.add_preserved_fd(mem_fd);
            let addr = bf.rangeStart as u64;
            let size = bf.size as u64;
            self.args([
                "--file-backed-mapping",
                &format!("{path},addr={addr:#0x},size={size:#0x},rw,ram"),
            ]);
        }
        Ok(())
    }

    fn add_assigned_devices_arg(&mut self, context: &RunContext) -> Result<()> {
        match &context.config.devices {
            // Non-VFIO case.
            aidl::AssignedDevices::Dtbo(Some(dtbo)) => {
                let fd = dtbo.as_ref().try_clone()?;
                let path = self.add_preserved_fd(fd);
                self.args(["--device-tree-overlay", &path.to_string()]);
            }
            // VFIO case.
            aidl::AssignedDevices::Devices(devices) if !devices.is_empty() => {
                if cfg!(early) {
                    bail!("VFIO devices not supported for early VMs yet");
                }

                // A simple sanity check
                let mut set = HashSet::new();
                for device in devices.iter() {
                    let path =
                        canonicalize(device).context(format!("can't canonicalize {device}"))?;
                    if !set.insert(path) {
                        return Err(anyhow!("duplicated device {device}"));
                    }
                }

                // Then bind
                let vfio_devices =
                    virtualmachine::global_service().unwrap().bindDevicesToVfioDriver(devices)?;

                const SYSFS_PLATFORM_DEVICES_PATH: &str = "/sys/devices/platform/";
                const VFIO_PLATFORM_DRIVER_PATH: &str = "/sys/bus/platform/drivers/vfio-platform";

                for device in vfio_devices.iter() {
                    // Check platform device exists
                    let path = Path::new(&device.getSysfsPath()?).canonicalize()?;
                    if !path.starts_with(SYSFS_PLATFORM_DEVICES_PATH) {
                        bail!("{path:?} is not a platform device");
                    }
                    // Check platform device is bound to VFIO driver
                    let dev_driver_path = path.join("driver").canonicalize()?;
                    if dev_driver_path != Path::new(VFIO_PLATFORM_DRIVER_PATH) {
                        bail!("{path:?} is not bound to VFIO-platform driver");
                    }

                    if let Some(p) = path.to_str() {
                        self.arg(format!(
                            "--vfio={p},iommu=pkvm-iommu,dt-symbol={0}",
                            device.getDtboLabel()?
                        ));
                    } else {
                        bail!("invalid path {path:?}");
                    }
                }

                let dtbo_fd = virtualmachine::global_service()
                    .unwrap()
                    .getDtboFile()?
                    .as_ref()
                    .try_clone()?;
                let path = self.add_preserved_fd(dtbo_fd);
                self.arg(format!("--device-tree-overlay={path},filter"));

                let cleaner = move |_: &CleanerContext| {
                    drop(vfio_devices); // Cleanup devices.
                    Ok(())
                };
                self.add_cleaner("vfio", Box::new(cleaner))?;
            }
            _ => (),
        };
        Ok(())
    }

    fn add_dump_dtb_arg(&mut self, context: &RunContext) -> Result<()> {
        let dump_dt_fd = if let Some(pfd) = context.devicetree_dump_out {
            pfd.as_ref().try_clone()?
        } else if context.debug_config.dump_device_tree {
            let path = context.temp_dir.join("device_tree.dtb");
            let file =
                File::create(path).context("Failed to prepare device tree dump file").with_log()?;
            file.into()
        } else {
            return Ok(());
        };

        let path = self.add_preserved_fd(dump_dt_fd);
        self.args(["--dump-device-tree-blob", &path]);
        Ok(())
    }

    fn add_gdb_arg(&mut self, context: &RunContext) -> Result<()> {
        if let Some(gdb_port) = NonZeroU16::new(context.config.gdbPort as u16) {
            if context.config.protectedVm {
                bail!("Can't use gdb with protected VMs");
            }
            if context.debug_config.debug_level == aidl::DebugLevel::NONE {
                bail!("Can't use gdb with non-deguggable VMs");
            }

            self.args(["--gdb", &gdb_port.to_string()]);
            self.args(["-p", "nokaslr"]);
        }
        Ok(())
    }

    fn add_teeservices_arg(&mut self, context: &RunContext) -> Result<()> {
        const GUEST_FFA_TEE_SERVICE: &str = "guest_ffa_tee_service";
        const KNOWN_TEE_SERVICES: [&str; 1] = [GUEST_FFA_TEE_SERVICE];

        for svc in &context.config.teeServices {
            let svc = svc.as_str();
            if svc.starts_with("vendor.") {
                continue;
            }
            if !KNOWN_TEE_SERVICES.contains(&svc) {
                bail!("Unknown tee_service {svc}");
            }

            if svc == GUEST_FFA_TEE_SERVICE {
                self.arg("--ffa=auto");
            }
        }
        Ok(())
    }
}

/// The lifecycle state which the payload in the VM has reported itself to be in.
///
/// Note that the order of enum variants is significant; only forward transitions are allowed by
/// [`VmInstance::update_payload_state`].
#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PayloadState {
    Starting,
    Started,
    Ready,
    Finished,
    Hangup, // Hasn't reached to Ready before timeout expires
}

/// The current state of the VM itself.
pub enum VmState {
    /// The VM has not yet tried to start.
    NotStarted {
        ///The configuration needed to start the VM, if it has not yet been started.
        config: Box<CrosvmConfig>,
    },
    /// The VM has been started.
    Running {
        /// The crosvm child process.
        child: Arc<SharedChild>,
        /// The thread waiting for crosvm to finish.
        monitor_vm_exit_thread: JoinHandle<()>,
    },
    /// The VM is being shut down.
    ShuttingDown {
        // The receiver half of this channel will be closed when shutdown is finished.
        shutdown_finished_tx: mpsc::SyncSender<()>,
    },
    /// The VM died or was killed.
    Dead,
    /// The VM failed to start.
    Failed,
}

impl std::fmt::Debug for VmState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted { .. } => f.write_str("not started"),
            Self::Running { .. } => f.write_str("running"),
            Self::ShuttingDown { .. } => f.write_str("shutting down"),
            Self::Dead => f.write_str("dead"),
            Self::Failed => f.write_str("failed"),
        }
    }
}

/// Metrics regarding the VM.
#[derive(Debug, Default)]
pub struct VmMetric {
    /// Recorded timestamp when the VM is started.
    pub start_timestamp: Option<SystemTime>,
    /// Cumulative guest CPU time measured before the VM is killed
    pub cpu_guest_time: Option<i64>,
    /// RSS high watermark measured before the VM is killed
    pub rss: Option<i64>,
}

impl VmState {
    /// Tries to start the VM, if it is in the `NotStarted` state.
    ///
    /// Returns an error if the VM is in the wrong state, or fails to start.
    fn start(&mut self, instance: Arc<VmInstance>) -> Result<(), Error> {
        let state = mem::replace(self, VmState::Failed);
        if let VmState::NotStarted { config } = state {
            let mut config = *config;
            let cleaners = config.command.cleaners.take().unwrap();
            let detect_hangup = config.detect_hangup;

            let vhost_fs_devices = run_virtiofs(&config)?;

            // If this fails and returns an error, `self` will be left in the `Failed` state.
            let child = Arc::new(run_vm(config, &instance.crosvm_control_socket_path)?);

            // It is too late to add a system API to control the ballooning behavior, so we
            // automically enable it only when the payload is granted
            // USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION permission as a temporarily
            // solution. TODO(b/407079334): Replace with SystemApi.
            let trim_under_pressure = instance.balloon_enabled
                && virtualmachine::check_use_relaxed_microdroid_rollback_protection().is_ok();

            let psi_thread_and_evt_fd = if trim_under_pressure {
                let psi_monitor_kill_event = Arc::new(EventFd::new()?);
                let psi_monitor_kill_event_clone = psi_monitor_kill_event.clone();
                let instance = instance.clone();
                Some((
                    thread::Builder::new().name("virt_psi_monitor".to_string()).spawn(
                        move || {
                            let mut expo_bo: u64 = 1;
                            // TODO: add metrics to see how often we restart the thread
                            while let Err(e) = psi_monitor(&instance, &psi_monitor_kill_event_clone)
                            {
                                error!("psi monitor failed: {e:#}");
                                virtualmachine::global_service()
                                    .unwrap()
                                    .forwardAtom(
                                        &aidl::Atom::PsiMonitorFailedReported(
                                            aidl::PsiMonitorFailedReported {
                                                exponentialBackoffSeconds: expo_bo
                                                    .try_into()
                                                    .unwrap(),
                                            },
                                        ),
                                        instance.requester_uid.try_into().unwrap(),
                                        &instance.name,
                                    )
                                    .unwrap_or_else(|e| {
                                        warn!("Failed to write PsiMonitorFailedReported atom: {e}");
                                    });
                                thread::sleep(Duration::from_secs(expo_bo));
                                // Exponential backoff, capped at 60 seconds. This number is
                                // arbitrary
                                expo_bo = min(expo_bo * 2, 60);
                            }
                        },
                    )?,
                    psi_monitor_kill_event,
                ))
            } else {
                None
            };

            let child_clone = child.clone();
            let instance_clone = instance.clone();
            let monitor_vm_exit_thread = thread::Builder::new()
                .name("vm_exit_monitor".to_string())
                .spawn(move || {
                    instance_clone.monitor_vm_exit(
                        child_clone,
                        vhost_fs_devices,
                        psi_thread_and_evt_fd,
                        cleaners,
                    );
                })
                .expect("Failed to create vm_exit_monitor thread");

            if detect_hangup {
                let child_clone = child.clone();
                thread::Builder::new()
                    .name("hangup_monitor".to_string())
                    .spawn(move || {
                        instance.monitor_payload_hangup(child_clone);
                    })
                    .expect("Failed to create hangup_monitor thread");
            }

            // If it started correctly, update the state.
            *self = VmState::Running { child, monitor_vm_exit_thread };
            Ok(())
        } else {
            *self = state;
            bail!("VM already started or failed")
        }
    }
}

fn trigger_trim(instance: &Arc<VmInstance>) {
    // When the host is under memory pressure, send a trim request
    // Full contention detected, send a trim request
    if let Some(guest_agent) = &*instance.guest_agent.lock().unwrap() {
        if let Err(e) = guest_agent.trimAsync() {
            error!("IGuestAgent::trimAsync failed: {e:#}");
        }
    }
}

fn psi_monitor(instance: &Arc<VmInstance>, psi_monitor_kill_event: &Arc<EventFd>) -> Result<()> {
    // monitor memory, inflate balloon if some contention exists
    // This will initialize a PSI monitor that monitors memory contention in
    // windows of 500_000us. If "Some" processes are stalled for a preiod of
    // 50_000us within the window period, an event gets fired.
    let mut memory_pressure_file =
        init_psi_monitor(PsiStallType::Some, 50000, 1000000, PsiResource::Memory)?;
    let epoll = Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)?;
    register_psi_monitor(&epoll, memory_pressure_file.as_fd(), 0)?;
    epoll
        .add(psi_monitor_kill_event.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 1))
        .context("failed to register psi eventfd")?;

    // Wait on event
    let mut events = [EpollEvent::empty()];
    let mut rate_limiter: Option<Instant> = None;
    let mut last_was_full = false;
    loop {
        // Set timeout to -1, blocking indefinitely
        // https://man7.org/linux/man-pages/man2/epoll_wait.2.html
        let epoll_res = epoll.wait(&mut events, EpollTimeout::NONE);
        if let Err(e) = epoll_res {
            if e == Errno::EINTR {
                // Ignore interrupts and wait again
                continue;
            } else {
                return Err(e.into());
            }
        }
        match events[0].data() {
            0 => {
                let mut psi_info = String::new();
                memory_pressure_file.rewind().context("failed to rewind file")?;
                memory_pressure_file
                    .read_to_string(&mut psi_info)
                    .context("Failed to read PSI monitor to buffer")?;
                // Monitor both Some and Full contention monitors.
                // If the system was not under memory contention, and then becomes under memory
                // contention, send a trim request directly.
                // If the system was under "Some" contention and went to "Full" contention, send a
                // trim request directly.
                // If the system was under memory contention and detected new contention, check if
                // timeout was hit.
                let full_stats = psi_info
                    .lines()
                    .filter_map(|l| parse_psi_line(l, PsiStallType::Full).ok())
                    .next();
                let some_stats = psi_info
                    .lines()
                    .filter_map(|l| parse_psi_line(l, PsiStallType::Some).ok())
                    .next();
                let full_triggered = full_stats.is_some() && full_stats.unwrap().avg10 > 0.0;
                let some_triggered = some_stats.is_some() && some_stats.unwrap().avg10 > 0.0;
                let is_rate_limited = rate_limiter.is_some()
                    && rate_limiter.unwrap().elapsed() > Duration::from_secs(22);

                let should_trim = if is_rate_limited {
                    !last_was_full && full_triggered
                } else {
                    full_triggered || some_triggered
                };

                last_was_full = full_triggered;

                if should_trim {
                    rate_limiter = Some(Instant::now());
                    trigger_trim(instance);
                }
            }
            1 => {
                info!("psi_monitor: Epoll kill event triggered");
                // EventFD triggered, return
                return Ok(());
            }
            _ => {
                return Err(anyhow!("Unknown event received: {:?}", events[0]));
            }
        }
    }
}

/// Information about a particular instance of a VM which may be running.
pub struct VmInstance {
    /// The current state of the VM.
    pub vm_state: Mutex<VmState>,
    /// Condvar that is notified when `vm_state` changes.
    vm_state_changed_condvar: Condvar,
    /// Whether this VmInstance requires VirtualMachineService
    pub requires_vm_service: bool,
    /// Hold the reference to RpcServer running VirtualMachineService
    pub vm_service: Mutex<Option<RpcServer>>,
    /// The CID assigned to the VM for vsock communication.
    pub cid: Cid,
    /// Path to crosvm control socket
    crosvm_control_socket_path: PathBuf,
    /// The name of the VM.
    pub name: String,
    /// Whether the VM is a protected VM.
    pub protected: bool,
    /// Directory of temporary files used by the VM while it is running.
    pub temporary_directory: PathBuf,
    /// The UID of the process which requested the VM.
    pub requester_uid: u32,
    /// The PID of the process which requested the VM. Note that this process may no longer exist
    /// and the PID may have been reused for a different process, so this should not be trusted.
    pub requester_debug_pid: i32,
    /// Callbacks to clients of the VM.
    pub callbacks: VirtualMachineCallbacks,
    /// Guest agent running on the VM
    pub guest_agent: Mutex<Option<Strong<dyn aidl::IGuestAgent>>>,
    /// Recorded metrics of VM such as timestamp or cpu / memory usage.
    pub vm_metric: Mutex<VmMetric>,
    // Whether virtio-balloon is enabled
    pub balloon_enabled: bool,
    /// List of vendor tee services this VM might access.
    pub vendor_tee_services: Vec<String>,
    /// List of host services this VM might access.
    pub host_services: Vec<String>,
    /// Represents a Key Encryption Key (KEK) stored on app's private data directory. This KEK is
    /// used to set up the encrypted store of guest.
    pub encrypted_store_kek: Option<Strong<dyn aidl::IEncryptedStoreKEK>>,
    /// The latest lifecycle state which the payload reported itself to be in.
    payload_state: Mutex<PayloadState>,
    /// Represents the condition that payload_state was updated
    payload_state_updated: Condvar,
    /// The human readable name of requester_uid
    requester_uid_name: String,
    /// Death recipient for the global service. (this doesn't implement Debug trait)
    pub global_service_death_recipient: Mutex<Option<DeathRecipient>>,
    /// Host console name
    pub host_console_name: Mutex<Option<String>>,
}

impl fmt::Display for VmInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let adj = if self.protected { "Protected" } else { "Non-protected" };
        write!(
            f,
            "{} virtual machine \"{}\" (owner: {}, cid: {})",
            adj, self.name, self.requester_uid_name, self.cid
        )
    }
}

impl VmInstance {
    /// Validates the given config and creates a new `VmInstance` but doesn't start running it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: CrosvmConfig,
        temporary_directory: PathBuf,
        requester_uid: u32,
        requester_debug_pid: i32,
        requires_vm_service: bool,
        vendor_tee_services: Vec<String>,
        host_services: Vec<String>,
        encrypted_store_kek: Option<Strong<dyn aidl::IEncryptedStoreKEK>>,
    ) -> Result<VmInstance, Error> {
        let cid = config.cid;
        let name = config.name.clone();
        let protected = config.protected;
        let balloon_enabled = config.command.balloon_enabled;
        let requester_uid_name = User::from_uid(Uid::from_raw(requester_uid))
            .ok()
            .flatten()
            .map_or_else(|| format!("{requester_uid}"), |u| u.name);
        let instance = VmInstance {
            vm_state: Mutex::new(VmState::NotStarted { config: Box::new(config) }),
            vm_state_changed_condvar: Condvar::new(),
            requires_vm_service,
            vm_service: Mutex::new(None),
            cid,
            crosvm_control_socket_path: temporary_directory.join("crosvm.sock"),
            name,
            protected,
            temporary_directory,
            requester_uid,
            requester_debug_pid,
            callbacks: Default::default(),
            guest_agent: Mutex::new(None),
            vm_metric: Mutex::new(Default::default()),
            payload_state: Mutex::new(PayloadState::Starting),
            payload_state_updated: Condvar::new(),
            requester_uid_name,
            balloon_enabled,
            vendor_tee_services,
            host_services,
            encrypted_store_kek,
            global_service_death_recipient: Mutex::new(None),
            host_console_name: Mutex::new(None),
        };
        info!("{} created", &instance);
        Ok(instance)
    }

    /// Starts an instance of `crosvm` to manage the VM. The `crosvm` instance will be killed when
    /// the `VmInstance` is dropped.
    pub fn start(self: &Arc<Self>) -> Result<(), Error> {
        let mut vm_metric = self.vm_metric.lock().unwrap();
        vm_metric.start_timestamp = Some(SystemTime::now());
        let ret = self.vm_state.lock().unwrap().start(self.clone());
        if ret.is_ok() {
            info!("{} started", &self);
        }
        ret.with_context(|| format!("{} failed to start", &self))
    }

    /// Monitors the exit of the VM (i.e. termination of the `child` process). When that happens,
    /// handles the event by updating the state, noityfing the event to clients by calling
    /// callbacks, and removing temporary files for the VM.
    fn monitor_vm_exit(
        &self,
        child: Arc<SharedChild>,
        vhost_user_devices: Vec<SharedChild>,
        psi_thread_and_evt_fd: Option<(JoinHandle<()>, Arc<EventFd>)>,
        cleaners: HashMap<String, Box<Cleaner>>,
    ) {
        // VirtualizationServiceInternal has a strong reference to IVirtualMachine. Don't forget to
        // delete it. Otherwise there'll be a memory leak. When cfg early is set, this is skipped
        // because early_virtmgr does not interact with the global virtualizationservice.
        #[cfg(not(early))]
        scopeguard::defer! {
            let cid = self.cid.try_into().unwrap();
            if let Err(e) = virtualmachine::global_service().unwrap().unregisterVirtualMachine(cid) {
                error!("Failed to unregister virtual machine ({cid}): {e:?}");
            }
        }

        // Wait for the EXIT of the crosvm process, but thanks to WNOWAIT it remains in the
        // waitable state so that we can inspect /proc/<pid>/stat or status. Note however that we
        // can only measure guest runtime, but not maximum RSS because VmHWM is not available for
        // zombie process.
        let pid = Pid::from_raw(child.id() as i32);
        let result = waitid(Id::Pid(pid), WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT);
        match &result {
            Err(e) => error!("Error waiting for crosvm({}) instance to die: {}", child.id(), e),
            Ok(WaitStatus::Exited(..)) | Ok(WaitStatus::Signaled(..)) => {
                self.measure_vm_status(child.id());
            }
            Ok(wait_status) => {
                error!("Unexpected wait status from crosvm({}): {:?}", child.id(), wait_status);
            }
        }

        // Then we really reap the process.
        let result = child.wait();
        match &result {
            Err(e) => error!("Error waiting for crosvm({}) instance to die: {}", child.id(), e),
            Ok(status) => {
                info!("crosvm({}) exited with status {}", child.id(), status);
                if let Some(exit_status_code) = status.code() {
                    if exit_status_code == CROSVM_WATCHDOG_REBOOT_STATUS {
                        info!("detected vcpu stall on crosvm");
                    }
                }
            }
        }

        let cleaner_context = CleanerContext { failure_reason: Mutex::new(String::new()) };
        cleaners.into_iter().for_each(|(name, cleaner)| {
            // Failure in a cleaner shouldn't stop running other cleaners.
            cleaner(&cleaner_context)
                .unwrap_or_else(|e| error!("Failed to run cleaner {name}: {e:?}"));
        });

        // In crosvm, when vhost_user frontend is dead, vhost_user backend device will detect and
        // exit. We can safely wait() for vhost user device after waiting crosvm main
        // process.
        for device in vhost_user_devices {
            match device.wait() {
                Ok(status) => {
                    info!("Vhost user device({}) exited with status {}", device.id(), status);
                    if !status.success() {
                        if let Some(code) = status.code() {
                            // vhost_user backend device exit with error code
                            error!(
                                "vhost user device({}) exited with error code: {}",
                                device.id(),
                                code
                            );
                        } else {
                            // The spawned child process of vhost_user backend device is
                            // killed by signal
                            error!("vhost user device({}) killed by signal", device.id());
                        }
                    }
                }
                Err(e) => {
                    error!("Error waiting for vhost user device({}) to die: {}", device.id(), e);
                }
            }
        }

        let failure_reason = cleaner_context.failure_reason.lock().unwrap();

        *self.vm_state.lock().unwrap() = VmState::Dead;
        self.vm_state_changed_condvar.notify_all();

        info!("{} exited", &self);

        // In case of hangup, the pipe doesn't give us any information because the hangup can't be
        // detected on the VM side (otherwise, it isn't a hangup), but in the
        // monitor_payload_hangup function below which updates the payload state to Hangup.
        let failure_reason =
            if failure_reason.is_empty() && self.payload_state() == PayloadState::Hangup {
                Cow::from("HANGUP")
            } else {
                Cow::from(failure_reason.clone())
            };

        self.handle_ramdump().unwrap_or_else(|e| error!("Error handling ramdump: {e}"));

        let death_reason = death_reason(&result, &failure_reason);
        let exit_signal = exit_signal(&result);

        self.callbacks.callback_on_died(self.cid, death_reason);

        let vm_metric = self.vm_metric.lock().unwrap();
        write_vm_exited_stats_sync(
            self.requester_uid as i32,
            &self.name,
            death_reason,
            exit_signal,
            &vm_metric,
        );

        if let Some((psi_thread, evt_fd)) = psi_thread_and_evt_fd {
            evt_fd.write(1).expect("failed to stop PSI thread");
            psi_thread.join().unwrap();
        }

        // Delete temporary files. The folder itself is removed by VirtualizationServiceInternal.
        virtualmachine::remove_temporary_files(&self.temporary_directory).unwrap_or_else(|e| {
            error!("Error removing temporary files from {:?}: {}", self.temporary_directory, e);
        });

        // Now that the VM is gone, shut down the VirtualMachineService server to eagerly free up
        // the server threads.
        let vm_service = self.vm_service.lock().unwrap();
        if let Some(service) = &*vm_service {
            if let Err(e) = service.shutdown() {
                error!("Failed to shutdown VirtualMachineService RPC Binder server: {e:#}");
            }
        }
    }

    /// Waits until payload is started, or timeout expires. When timeout occurs, kill
    /// the VM to prevent indefinite hangup and update the payload_state accordingly.
    fn monitor_payload_hangup(&self, child: Arc<SharedChild>) {
        debug!("Starting to monitor hangup for Microdroid({})", child.id());
        let (state, result) = self
            .payload_state_updated
            .wait_timeout_while(self.payload_state.lock().unwrap(), *BOOT_HANGUP_TIMEOUT, |s| {
                *s < PayloadState::Started
            })
            .unwrap();
        drop(state); // we are not interested in state
        let child_still_running = child.try_wait().ok() == Some(None);
        if result.timed_out() && child_still_running {
            error!(
                "Microdroid({}) failed to start payload within {} secs timeout. Shutting down.",
                child.id(),
                BOOT_HANGUP_TIMEOUT.as_secs()
            );
            self.update_payload_state(PayloadState::Hangup).unwrap();
            if let Err(e) = self.kill() {
                error!("Error stopping timed-out VM with CID {}: {:?}", child.id(), e);
            }
        }
    }

    fn measure_vm_status(&self, pid: u32) {
        match get_guest_time(pid) {
            Ok(guest_time) => self.vm_metric.lock().unwrap().cpu_guest_time = Some(guest_time),
            Err(e) => warn!("Failed to get guest CPU time: {e}"),
        }

        match get_rss(pid) {
            Ok(rss) => self.vm_metric.lock().unwrap().rss = Some(rss),
            Err(e) => warn!("Failed to get guest RSS: {e}"),
        }
    }

    /// Tells if VM is running or not
    pub fn is_vm_running(&self) -> bool {
        matches!(&*self.vm_state.lock().unwrap(), VmState::Running { .. })
    }

    /// Returns the last reported state of the VM payload.
    pub fn payload_state(&self) -> PayloadState {
        *self.payload_state.lock().unwrap()
    }

    /// Updates the payload state to the given value, if it is a valid state transition.
    pub fn update_payload_state(&self, new_state: PayloadState) -> Result<(), Error> {
        if new_state == PayloadState::Finished {
            if let VmState::Running { child, .. } = &*self.vm_state.lock().unwrap() {
                self.measure_vm_status(child.id());
            }
        }

        let mut state_locked = self.payload_state.lock().unwrap();
        // Only allow forward transitions, e.g. from starting to started or finished, not back in
        // the other direction.
        if new_state > *state_locked {
            *state_locked = new_state;
            self.payload_state_updated.notify_all();
            Ok(())
        } else {
            bail!("Invalid payload state transition from {:?} to {:?}", *state_locked, new_state)
        }
    }

    fn try_shutdown(&self) -> bool {
        if let Some(guest_agent) = &*self.guest_agent.lock().unwrap() {
            info!("Asking VM (name: {}, cid: {}) to shut down", self.name, self.cid);
            // If a guest agent was registered, the VM is assumed to support graceful shutdown.
            // Return true to give it more time to shutdown.
            let _ =
                guest_agent.shutdownAsync().map_err(|e| error!("Failed to ask shut down: {e:?}"));
            return true;
        }
        false
    }

    /// Kills the crosvm instance, if it is running. We try to shut it down gracefully, if guest
    /// agent is installed there. If not, or the shutdown didn't finish on time, the VM is forcibly
    /// shut down. In-flight data in the VM may be affected!
    pub fn kill(&self) -> Result<(), Error> {
        let mut vm_state_mg = self.vm_state.lock().unwrap();
        match &*vm_state_mg {
            VmState::Running { .. } => {
                // We use an `mpsc` in a backwards way as a poor man's broadcast channel. The
                // buffer is set to 0 to make this into a "rendezvous channel". Code that wants to
                // wait for shutdown to finish will `send` on the channel, which will block until
                // we `recv` (we never do) or `drop`.
                let (shutdown_finished_tx, shutdown_finished_rx) = mpsc::sync_channel(0);

                let vm_state = std::mem::replace(
                    &mut *vm_state_mg,
                    VmState::ShuttingDown { shutdown_finished_tx },
                );
                drop(vm_state_mg); // make sure self.vm_state is not held

                let VmState::Running { child, monitor_vm_exit_thread } = vm_state else {
                    unreachable!();
                };

                self.measure_vm_status(child.id());

                if !self.try_shutdown() {
                    let id = child.id();
                    warn!(
                        "Killing VM (name: {}, cid: {}) forcibly. Data might be corrupted!!!",
                        self.name, self.cid
                    );
                    child.kill().with_context(|| format!("Error killing crosvm({id}) instance"))?;
                }

                // Wait until the VM moves out of the ShuttingDown state. When the VM is shut down
                // or killed, the state is set to Dead. See monitor_vm_exit_thread.
                let shutdown_timeout = Duration::from_secs(5);
                let result = self
                    .vm_state_changed_condvar
                    .wait_timeout_while(self.vm_state.lock().unwrap(), shutdown_timeout, |state| {
                        matches!(state, VmState::ShuttingDown { .. })
                    })
                    .unwrap();
                if result.1.timed_out() {
                    warn!(
                        "Failed to shut down the VM in {shutdown_timeout:?}. Killing. Data might be corrupted!."
                    );
                    child.kill().unwrap();
                }
                drop(result); // unlock self.vm_state to avoid deadlock with the vm_exit thread.

                // Wait once again. If the graceful shutdown was successful, this will return
                // immediately.
                monitor_vm_exit_thread.join().unwrap();

                // Drop the channel to signal shutdown is finished.
                // Done explicitly just for code visibility.
                drop(shutdown_finished_rx);
            }
            VmState::ShuttingDown { shutdown_finished_tx } => {
                let shutdown_finished_tx = shutdown_finished_tx.clone();
                drop(vm_state_mg); // make sure self.vm_state is not held

                // Wait for the shutdown to finish.
                //
                // We might consider adding a timeout here just in case, but note that, if this has
                // a case where it blocks indefinitely, then the `Running` branch above must have
                // such a case a well (because it never dropped the other half).
                #[allow(clippy::single_match)]
                match shutdown_finished_tx.send(()) {
                    Ok(()) => unreachable!(),
                    Err(mpsc::SendError(())) => {} // success!
                }
            }
            VmState::NotStarted { .. } | VmState::Dead | VmState::Failed => {
                drop(vm_state_mg); // make sure self.vm_state is not held

                // TODO: if it were ever running, we may still need to join
                // logging handles, in monitor_vm_exit.
                bail!("VM is not running")
            }
        }

        Ok(())
    }

    /// Returns current virtio-balloon size.
    pub fn get_actual_memory_balloon_bytes(&self) -> Result<u64, Error> {
        Ok(self.get_balloon_stats()?.0)
    }

    fn get_balloon_stats(&self) -> Result<(u64, crosvm_control::BalloonStatsFfi), Error> {
        if !self.is_vm_running() {
            bail!("get_actual_memory_balloon_bytes when VM is not running");
        }
        if !self.balloon_enabled {
            bail!("virtio-balloon is not enabled");
        }
        let socket_path_cstring = path_to_cstring(&self.crosvm_control_socket_path);
        let mut stats = crosvm_control::BalloonStatsFfi {
            swap_in: 0,
            swap_out: 0,
            major_faults: 0,
            minor_faults: 0,
            free_memory: 0,
            total_memory: 0,
            available_memory: 0,
            disk_caches: 0,
            hugetlb_allocations: 0,
            hugetlb_failures: 0,
            shared_memory: 0,
            unevictable_memory: 0,
        };
        let mut balloon_actual = 0u64;
        // SAFETY: Pointers are valid for the lifetime of the call.
        let success = unsafe {
            crosvm_control::crosvm_client_balloon_stats(
                socket_path_cstring.as_ptr(),
                &mut stats,
                &mut balloon_actual,
            )
        };
        if !success {
            bail!("Error requesting balloon stats");
        }
        Ok((balloon_actual, stats))
    }

    /// Inflates the virtio-balloon to `num_bytes` to reclaim guest memory. Called in response to
    /// memory-trimming notifications.
    pub fn set_memory_balloon(&self, num_bytes: u64) -> Result<(), Error> {
        if !self.is_vm_running() {
            bail!("set_memory_balloon when VM is not running");
        }
        if !self.balloon_enabled {
            bail!("virtio-balloon is not enabled");
        }
        let socket_path_cstring = path_to_cstring(&self.crosvm_control_socket_path);
        // SAFETY: Pointer is valid for the lifetime of the call.
        let success = unsafe {
            crosvm_control::crosvm_client_balloon_vms(socket_path_cstring.as_ptr(), num_bytes)
        };
        if !success {
            bail!("Error sending balloon adjustment");
        }
        Ok(())
    }

    /// Checks if ramdump has been created. If so, send it to tombstoned.
    fn handle_ramdump(&self) -> Result<(), Error> {
        let ramdump_path = self.temporary_directory.join("ramdump");
        if !ramdump_path.as_path().try_exists()? {
            return Ok(());
        }
        if std::fs::metadata(&ramdump_path)?.len() > 0 {
            Self::send_ramdump_to_tombstoned(&ramdump_path)?;
        }
        Ok(())
    }

    fn send_ramdump_to_tombstoned(ramdump_path: &Path) -> Result<(), Error> {
        let mut input = File::open(ramdump_path)
            .context(format!("Failed to open ramdump {ramdump_path:?} for reading"))?;

        let pid = std::process::id() as i32;
        let conn = TombstonedConnection::connect(pid, DebuggerdDumpType::Tombstone)
            .context("Failed to connect to tombstoned")?;
        let mut output = conn
            .text_output
            .as_ref()
            .ok_or_else(|| anyhow!("Could not get file to write the tombstones on"))?;

        std::io::copy(&mut input, &mut output).context("Failed to send ramdump to tombstoned")?;
        info!("Ramdump {ramdump_path:?} sent to tombstoned");

        conn.notify_completion()?;
        Ok(())
    }

    /// Suspends the VM's vCPUs.
    pub fn suspend(&self) -> Result<(), Error> {
        let socket_path_cstring = path_to_cstring(&self.crosvm_control_socket_path);
        // SAFETY: Pointer is valid for the lifetime of the call.
        let success =
            unsafe { crosvm_control::crosvm_client_suspend_vm(socket_path_cstring.as_ptr()) };
        if !success {
            bail!("Failed to suspend VM");
        }
        Ok(())
    }

    /// Resumes the VM's vCPUs.
    pub fn resume(&self) -> Result<(), Error> {
        let socket_path_cstring = path_to_cstring(&self.crosvm_control_socket_path);
        // SAFETY: Pointer is valid for the lifetime of the call.
        let success =
            unsafe { crosvm_control::crosvm_client_resume_vm(socket_path_cstring.as_ptr()) };
        if !success {
            bail!("Failed to resume VM");
        }
        Ok(())
    }

    /// Performs full resume of VM.
    pub fn resume_full(&self) -> Result<(), Error> {
        let socket_path_cstring = path_to_cstring(&self.crosvm_control_socket_path);
        // SAFETY: Pointer is valid for the lifetime of the call.
        let success =
            unsafe { crosvm_control::crosvm_client_resume_vm_full(socket_path_cstring.as_ptr()) };
        if !success {
            bail!("Failed to resume VM");
        }
        Ok(())
    }

    /// Sets the guest agent for this VM.
    pub fn set_guest_agent(&self, guest_agent: &Strong<dyn aidl::IGuestAgent>) {
        *self.guest_agent.lock().unwrap() = Some(guest_agent.clone());
        self.vm_state_changed_condvar.notify_all();
        info!("VM with CID {} has registered a guest agent", self.cid);
    }
}

// Get Cpus_allowed mask
fn check_if_all_cpus_allowed() -> Result<bool> {
    let file = read_to_string("/proc/self/status")?;
    let lines: Vec<_> = file.split('\n').collect();

    for line in lines {
        if line.contains("Cpus_allowed_list") {
            let prop: Vec<_> = line.split_whitespace().collect();
            if prop.len() != 2 {
                return Ok(false);
            }
            let cpu_list: Vec<_> = prop[1].split('-').collect();
            //Only contiguous Cpu list allowed
            if cpu_list.len() != 2 {
                return Ok(false);
            }
            if let Some(cpus) = get_num_cpus() {
                let max_cpu = cpu_list[1].parse::<usize>()?;
                if max_cpu == cpus - 1 {
                    return Ok(true);
                } else {
                    return Ok(false);
                }
            }
        }
    }
    Ok(false)
}

// Get guest time from /proc/[crosvm pid]/stat
fn get_guest_time(pid: u32) -> Result<i64> {
    let file = read_to_string(format!("/proc/{pid}/stat"))?;
    let data_list: Vec<_> = file.split_whitespace().collect();

    // Information about guest_time is at 43th place of the file split with the whitespace.
    // Example of /proc/[pid]/stat :
    // 6603 (kworker/104:1H-kblockd) I 2 0 0 0 -1 69238880 0 0 0 0 0 88 0 0 0 -20 1 0 1845 0 0
    // 18446744073709551615 0 0 0 0 0 0 0 2147483647 0 0 0 0 17 104 0 0 0 0 0 0 0 0 0 0 0 0 0
    if data_list.len() < 43 {
        bail!("Failed to parse command result for getting guest time : {}", file);
    }

    let guest_time_ticks = data_list[42].parse::<i64>()?;
    if guest_time_ticks == 0 {
        bail!("zero value is measured on elapsed CPU guest_time");
    }
    // SAFETY: It just returns an integer about CPU tick information.
    let ticks_per_sec = unsafe { sysconf(_SC_CLK_TCK) };
    Ok(guest_time_ticks * MILLIS_PER_SEC / ticks_per_sec)
}

// Get rss from VmHWM of /proc/[crosvm pid]/status
fn get_rss(pid: u32) -> Result<i64> {
    let file = read_to_string(format!("/proc/{pid}/status"))?;
    let lines: Vec<_> = file.split('\n').collect();

    for line in lines {
        // VmHWM:  12345 kB
        if line.starts_with("VmHWM:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() != 3 {
                bail!("Failed to parse line: {}", line);
            }
            let rss = parts[1].parse::<i64>()?;
            // We no longer distinguish memory used by the VM itself and the containint crosvm
            // process. The former is not available as /proc/<pid>/smaps is not available when
            // the process is in zombie state.
            return Ok(rss);
        }
    }
    bail!("can't find VmHWM in the status file");
}

fn death_reason(
    result: &Result<ExitStatus, io::Error>,
    mut failure_reason: &str,
) -> aidl::DeathReason {
    use aidl::DeathReason;

    if let Some((reason, info)) = failure_reason.split_once('|') {
        // Separator indicates extra context information is present after the failure name.
        error!("Failure info: {info}");
        failure_reason = reason;
    }
    if let Ok(status) = result {
        match failure_reason {
            "PVM_FIRMWARE_PUBLIC_KEY_MISMATCH" => {
                return DeathReason::PVM_FIRMWARE_PUBLIC_KEY_MISMATCH
            }
            "PVM_FIRMWARE_INSTANCE_IMAGE_CHANGED" => {
                return DeathReason::PVM_FIRMWARE_INSTANCE_IMAGE_CHANGED
            }
            "MICRODROID_FAILED_TO_CONNECT_TO_VIRTUALIZATION_SERVICE" => {
                return DeathReason::MICRODROID_FAILED_TO_CONNECT_TO_VIRTUALIZATION_SERVICE
            }
            "MICRODROID_PAYLOAD_HAS_CHANGED" => return DeathReason::MICRODROID_PAYLOAD_HAS_CHANGED,
            "MICRODROID_PAYLOAD_VERIFICATION_FAILED" => {
                return DeathReason::MICRODROID_PAYLOAD_VERIFICATION_FAILED
            }
            "MICRODROID_INVALID_PAYLOAD_CONFIG" => {
                return DeathReason::MICRODROID_INVALID_PAYLOAD_CONFIG
            }
            "MICRODROID_UNKNOWN_RUNTIME_ERROR" => {
                return DeathReason::MICRODROID_UNKNOWN_RUNTIME_ERROR
            }
            "HANGUP" => return DeathReason::HANGUP,
            _ => {}
        }
        match status.code() {
            None => DeathReason::KILLED,
            Some(0) => DeathReason::SHUTDOWN,
            Some(CROSVM_START_ERROR_STATUS) => DeathReason::START_FAILED,
            Some(CROSVM_REBOOT_STATUS) => DeathReason::REBOOT,
            Some(CROSVM_CRASH_STATUS) => DeathReason::CRASH,
            Some(CROSVM_WATCHDOG_REBOOT_STATUS) => DeathReason::WATCHDOG_REBOOT,
            Some(_) => DeathReason::UNKNOWN,
        }
    } else {
        DeathReason::INFRASTRUCTURE_ERROR
    }
}

fn exit_signal(result: &Result<ExitStatus, io::Error>) -> Option<i32> {
    match result {
        Ok(status) => status.signal(),
        Err(_) => None,
    }
}

fn run_virtiofs(config: &CrosvmConfig) -> io::Result<Vec<SharedChild>> {
    let mut devices: Vec<SharedChild> = Vec::new();
    for shared_path in &config.shared_paths {
        if shared_path.app_domain {
            continue;
        }
        let ugid_map_value = format!(
            "{} {} {} {} {} /; {} {} {} {} {} /Android",
            shared_path.guest_uid,
            shared_path.guest_gid,
            shared_path.host_uid,
            shared_path.host_gid,
            shared_path.mask,
            0,
            0,
            0,
            0,
            0o777,
        );

        let mut cfg_arg = format!("ugid_map='{ugid_map_value}'");
        if config.protected {
            cfg_arg.push_str(",unmap_guest_memory_on_fork=true");
        }

        let mut command = Command::new(CROSVM_PATH);
        command
            .arg("device")
            .arg("fs")
            .arg(format!("--socket={}", &shared_path.socket_path))
            .arg(format!("--tag={}", &shared_path.tag))
            .arg(format!("--shared-dir={}", &shared_path.path))
            .arg("--cfg")
            .arg(cfg_arg.as_str())
            .arg("--disable-sandbox")
            .arg("--skip-pivot-root=true");

        print_crosvm_args(&command);

        let result = SharedChild::spawn(&mut command)?;
        info!("Spawned virtiofs crosvm({})", result.id());
        devices.push(result);
    }

    Ok(devices)
}

/// Starts an instance of `crosvm` to manage a new VM.
fn run_vm(config: CrosvmConfig, crosvm_control_socket_path: &Path) -> Result<SharedChild, Error> {
    let mut command = Command::new(CROSVM_PATH);

    command.arg0(config.command.arg0);
    command.args(config.command.args);

    // Keep track of what file descriptors should be mapped to the crosvm process.
    let mut preserved_fds = Vec::new();
    preserved_fds.extend(config.command.preserved_fds);

    let control_sock = create_crosvm_control_listener(crosvm_control_socket_path)
        .context("failed to create control listener")?;
    command.arg("--socket").arg(add_preserved_fd(&mut preserved_fds, control_sock));

    config.device_tree_overlays.into_iter().for_each(|dt_overlay| {
        let arg = add_preserved_fd(&mut preserved_fds, dt_overlay);
        command.arg("--device-tree-overlay").arg(arg);
    });

    for shared_path in &config.shared_paths {
        if shared_path.app_domain {
            if let Some(socket_fd) = &shared_path.socket_fd {
                let socket_path =
                    add_preserved_fd(&mut preserved_fds, socket_fd.try_clone().unwrap());
                command.arg("--vhost-user").arg(format!("fs,socket={socket_path}"));
            }
        } else {
            if let Err(e) = wait_for_file(&shared_path.socket_path, 5) {
                bail!("Error waiting for file: {}", e);
            }
            command.arg("--vhost-user").arg(format!("fs,socket={}", shared_path.socket_path));
        }
    }

    debug!("Preserving FDs {preserved_fds:?}");
    command.preserved_fds(preserved_fds);

    if config.start_suspended {
        command.arg("--suspended");
    }

    print_crosvm_args(&command);

    let result = SharedChild::spawn(&mut command)?;
    debug!("Spawned crosvm({}).", result.id());
    Ok(result)
}

fn wait_for_file(path: &str, timeout_secs: u64) -> Result<(), std::io::Error> {
    let start_time = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);

    while start_time.elapsed() < timeout {
        if std::fs::metadata(path).is_ok() {
            return Ok(()); // File exists
        }
        thread::sleep(Duration::from_millis(100));
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("File not found within {timeout_secs} seconds: {path}"),
    ))
}

/// Print arguments of the crosvm command. In doing so, /proc/self/fd/XX is annotated with the
/// actual file path if the FD is backed by a regular file. If not, the /proc path is printed
/// unmodified.
fn print_crosvm_args(command: &Command) {
    let re = Regex::new(r"/proc/self/fd/[\d]+").unwrap();
    info!(
        "Running crosvm with args: {:?}",
        command
            .get_args()
            .map(|s| s.to_string_lossy())
            .map(|s| {
                re.replace_all(&s, |caps: &Captures| {
                    let path = &caps[0];
                    if let Ok(link_target) = std::fs::read_link(path) {
                        format!("{} ({})", path, link_target.to_string_lossy())
                    } else {
                        path.to_owned()
                    }
                })
                .into_owned()
            })
            .collect::<Vec<_>>()
    );
}

/// Adds the file descriptor for `file` to `preserved_fds`, and returns a string of the form
/// "/proc/self/fd/N" where N is the file descriptor.
fn add_preserved_fd<F: Into<OwnedFd>>(preserved_fds: &mut Vec<OwnedFd>, file: F) -> String {
    let fd = file.into();
    let raw_fd = fd.as_raw_fd();
    preserved_fds.push(fd);
    format!("/proc/self/fd/{raw_fd}")
}

/// Creates a new pipe with the `O_CLOEXEC` flag set, and returns the read side and write side.
fn create_pipe() -> Result<(File, File), Error> {
    let (read_fd, write_fd) = pipe2(OFlag::O_CLOEXEC)?;
    Ok((read_fd.into(), write_fd.into()))
}

/// Creates and binds a unix seqpacket listening socket to be passed as crosvm's `--socket`
/// argument. See `UnixSeqpacketListener::bind` in crosvm's code for reference.
fn create_crosvm_control_listener(crosvm_control_socket_path: &Path) -> Result<OwnedFd> {
    use nix::sys::socket;
    let fd = socket::socket(
        socket::AddressFamily::Unix,
        socket::SockType::SeqPacket,
        socket::SockFlag::empty(),
        None,
    )
    .context("socket failed")?;
    socket::bind(fd.as_raw_fd(), &socket::UnixAddr::new(crosvm_control_socket_path)?)
        .context("bind failed")?;
    // The exact backlog size isn't imporant. crosvm uses 128 internally. We use 127 here
    // because of a `nix` bug.
    socket::listen(&fd, socket::Backlog::new(127).unwrap()).context("listen failed")?;
    Ok(fd)
}

fn path_to_cstring(path: &Path) -> CString {
    if let Some(s) = path.to_str() {
        if let Ok(s) = CString::new(s) {
            return s;
        }
    }
    // The path contains invalid utf8 or a null, which should never happen.
    panic!("bad path: {path:?}");
}

// This is a duplicate of the check done by pvmfw.
// We do it in virtmgr just to fail fast without even trying to boot a VM with invalid name.
fn is_valid_vm_name(name: &str) -> bool {
    name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

struct SwiotlbEstimateInputs {
    guest_page_size: u32,
    block_count: u32,
    console_count: u32,
    balloon: bool,
}

/// Estimate needed size of SWIOTLB based on crosvm and Linux kernel implementation details and
/// workload guesses.
///
/// Since it is based on implementations details of other projects, it is bound to go stale.
///
/// Optimized for microdroid. Custom VMs may want to set an explicit swiotlb size in their
/// configs.
fn estimate_swiotlb_usage_mib(inputs: SwiotlbEstimateInputs) -> u32 {
    fn align(x: u32, alignment: u32) -> u32 {
        (x + alignment) & alignment
    }
    // virtio split queue data structure size, based on virtio spec.
    let virtq_size = |entries: u32| -> u32 {
        // Assume any extra space in the last page is wasted.
        align(
            align(16 * entries, 16) + align(6 + 2 * entries, 2) + align(6 + 8 * entries, 4),
            inputs.guest_page_size,
        )
    };

    let mut total = 0;

    // virtio vsock.
    total += [
        // event queue.
        virtq_size(256),
        // tx queue.
        virtq_size(256),
        // rx queue.
        virtq_size(256),
        // Linux eagerly fills the rx queue with requests, one page each.
        256 * inputs.guest_page_size,
    ]
    .iter()
    .sum::<u32>();

    // virtio console.
    total += inputs.console_count
        * [
            // tx queue.
            virtq_size(CONSOLE_TX_QUEUE_SIZE),
            // rx queue.
            virtq_size(CONSOLE_RX_QUEUE_SIZE),
            // Linux eagerly fills the rx queue with requests, one page each.
            CONSOLE_RX_QUEUE_SIZE * inputs.guest_page_size,
        ]
        .iter()
        .sum::<u32>();

    // virtio block.
    total += inputs.block_count
        * [
            // crosvm gives 16 queues.
            16 * virtq_size(256),
        ]
        .iter()
        .sum::<u32>();

    // virtio balloon.
    if inputs.balloon {
        // Expected queues: inflate, deflate, stats, reporting
        total += 4 * virtq_size(128);
    }

    // Guess at workload dependant peak memory needs.
    //
    // This was chosen by making it just large enough to boot Microdroid, then adding 2 MiB. Maybe
    // should add more based on vCPU count and/or page size.
    total += 4 * 1024 * 1024;

    total.div_ceil(1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_swiotlb() {
        // Basic microdroid configuration.
        assert_eq!(
            estimate_swiotlb_usage_mib(SwiotlbEstimateInputs {
                guest_page_size: 4096,
                block_count: 3,
                console_count: 3,
                balloon: true,
            }),
            6
        );
        // Basic 16k microdroid configuration.
        assert_eq!(
            estimate_swiotlb_usage_mib(SwiotlbEstimateInputs {
                guest_page_size: 16 * 1024,
                block_count: 3,
                console_count: 3,
                balloon: true,
            }),
            10
        );
    }

    #[test]
    fn test_is_valid_vm_name() {
        assert!(is_valid_vm_name("val_id-na42me"));
        assert!(!is_valid_vm_name("\\invalid_%name%"));
        assert!(!is_valid_vm_name("a_🐸_in_vm_name"));
    }
}
