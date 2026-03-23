// Copyright 2025 The Android Open Source Project
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

//! VTS tests for pkvm hypervisor tracing interface.

use anyhow::{anyhow, Context, Result};
use log::info;
use rdroidtest::{ignore_if, rdroidtest};
use std::fs::OpenOptions;
use std::io::prelude::*;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Return version of kernel running on the device.
// TODO(ioffe): consider moving this somewhere under libs/
fn get_kernel_version() -> Result<(u32, u32)> {
    let release = nix::sys::utsname::uname()?.release().to_string_lossy().into_owned();
    let mut iter = release.splitn(3, ".");
    let major = iter
        .next()
        .ok_or(anyhow!("missing major version"))?
        .parse::<u32>()
        .context("failed to parse major version")?;
    let minor = iter
        .next()
        .ok_or(anyhow!("missing minor version"))?
        .parse::<u32>()
        .context("failed to parse minor version")?;
    Ok((major, minor))
}

struct HypTracingInstance {
    root_path: PathBuf,
    instance_name: String,
}

impl HypTracingInstance {
    fn new(instance_name: &str) -> Self {
        HypTracingInstance {
            root_path: Path::new("/sys/kernel/tracing/").join(instance_name),
            instance_name: instance_name.to_owned(),
        }
    }

    fn root(&self) -> &Path {
        &self.root_path
    }

    fn events(&self) -> WalkDir {
        WalkDir::new(Path::new(&self.root_path).join("events").join(self.instance_name.clone()))
            .min_depth(1)
            .max_depth(1)
    }

    fn per_cpu(&self) -> WalkDir {
        WalkDir::new(Path::new(&self.root_path).join("per_cpu")).min_depth(1).max_depth(1)
    }

    fn enable_tracing(&self, enable: bool) -> Result<()> {
        let mut tracing_on_file = OpenOptions::new()
            .write(true)
            .open(Path::new(&self.root_path).join("tracing_on"))
            .context("failed to open tracing_on")?;
        tracing_on_file
            .write(if enable { b"1" } else { b"0" })
            .context("failed to write to tracing_on")?;
        Ok(())
    }

    fn enable_event(&self, event_name: &str, enable: bool) -> Result<()> {
        let mut event_file = OpenOptions::new()
            .write(true)
            .open(
                Path::new(&self.root_path)
                    .join("events")
                    .join(self.instance_name.clone())
                    .join(event_name)
                    .join("enable"),
            )
            .context("failed to open tracing_on")?;
        event_file
            .write(if enable { b"1" } else { b"0" })
            .context("failed to write to {event_name}/enable")?;
        Ok(())
    }
}

/// Returns expected patch for the hypervisor tracing instance depending on the kernel version
fn hyp_tracing_instance() -> Result<HypTracingInstance> {
    let kernel_version = get_kernel_version().context("uname failed")?;
    if kernel_version <= (6, 6) {
        Ok(HypTracingInstance::new("hyp"))
    } else {
        Ok(HypTracingInstance::new("hypervisor"))
    }
}

#[rdroidtest]
#[ignore_if(!hypervisor_props::is_pkvm().unwrap_or_default())]
fn test_hyp_tracing_interface_exists() {
    let hyp_tracing_instance = hyp_tracing_instance().unwrap();
    assert!(std::fs::exists(hyp_tracing_instance.root()).unwrap());
    // TODO(ioffe): also check that tracing path has correct selinux domain.
}

#[rdroidtest]
#[ignore_if(!hypervisor_props::is_pkvm().unwrap_or_default())]
fn test_hyp_tracing_events() {
    let hyp_tracing_instance = hyp_tracing_instance().unwrap();
    for event_entry in hyp_tracing_instance.events() {
        let event = event_entry.unwrap();
        let id_path = Path::new(event.path()).join("id");
        info!("checking event {}", id_path.display());
        let mut id_file = OpenOptions::new().read(true).open(id_path).unwrap();
        let mut contents = String::new();
        id_file.read_to_string(&mut contents).unwrap();
        info!("contents: {contents}");
        let id = contents.trim().parse::<u32>().unwrap();
        // Perfetto expects ids to be positive, howerver it's fine for __hyp_printk to have zero as
        // id, since we will never use it in perfetto tracing configs.
        assert!(id > 0 || event.file_name().to_string_lossy() == "__hyp_printk");
    }
}

#[rdroidtest]
#[ignore_if(!hypervisor_props::is_pkvm().unwrap_or_default())]
fn test_hyp_tracing_trace_files() {
    fn test_trace_file(trace_path: &Path) {
        info!("checking {}", trace_path.display());
        // Try all options: open just for read, just for write and for read+write.
        OpenOptions::new().read(true).open(trace_path).unwrap();
        OpenOptions::new().write(true).open(trace_path).unwrap();
        OpenOptions::new().read(true).write(true).open(trace_path).unwrap();
    }

    let hyp_tracing_instance = hyp_tracing_instance().unwrap();

    let global_trace_path = Path::new(hyp_tracing_instance.root()).join("trace").to_path_buf();
    test_trace_file(&global_trace_path);
    for per_cpu_entry in hyp_tracing_instance.per_cpu() {
        let per_cpu = per_cpu_entry.unwrap();
        let per_cpu_trace_path = Path::new(per_cpu.path()).join("trace").to_path_buf();
        test_trace_file(&per_cpu_trace_path);
    }
}

#[rdroidtest]
#[ignore_if(!hypervisor_props::is_pkvm().unwrap_or_default())]
fn test_hyp_tracing_trace_raw_pipe() {
    let hyp_tracing_instance = hyp_tracing_instance().unwrap();

    hyp_tracing_instance.enable_tracing(true).unwrap();
    scopeguard::defer!({
        hyp_tracing_instance.enable_tracing(false).unwrap();
        hyp_tracing_instance.enable_event("hyp_enter", false).unwrap();
        hyp_tracing_instance.enable_event("hyp_exit", false).unwrap();
        hyp_tracing_instance.enable_event("host_hcall", false).unwrap();
    });

    // Enable some events so that we can read something from the raw pipes.
    hyp_tracing_instance.enable_event("hyp_enter", true).unwrap();
    hyp_tracing_instance.enable_event("hyp_exit", true).unwrap();
    hyp_tracing_instance.enable_event("host_hcall", true).unwrap();

    // TODO(ioffe): make reading from raw_trace_pipe async
    for per_cpu_entry in hyp_tracing_instance.per_cpu() {
        let per_cpu = per_cpu_entry.unwrap();
        let trace_pipe_raw_path = Path::new(per_cpu.path()).join("trace_pipe_raw");
        info!("testing {}", trace_pipe_raw_path.display());
        let mut trace_pipe_raw = OpenOptions::new().read(true).open(trace_pipe_raw_path).unwrap();
        let mut buf = [0; 8192];
        trace_pipe_raw.read_exact(&mut buf).unwrap();
    }
}

rdroidtest::test_main!();
