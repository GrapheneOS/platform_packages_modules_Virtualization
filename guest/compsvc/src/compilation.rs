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

use anyhow::{anyhow, bail, Context, Result};
use log::{debug, info, warn};
use minijail::{self, Minijail};
use regex::Regex;
use rustutils::system_properties;
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::path::{self, Path, PathBuf};
use std::process::Command;

use authfs_aidl_interface::aidl::com::android::virt::fs::{
    AuthFsConfig::{
        AuthFsConfig, InputDirFdAnnotation::InputDirFdAnnotation,
        OutputDirFdAnnotation::OutputDirFdAnnotation,
    },
    IAuthFsService::IAuthFsService,
};
use binder::Strong;
use compos_aidl_interface::aidl::com::android::compos::ICompOsService::{
    CompilationMode::CompilationMode, OdrefreshArgs::OdrefreshArgs,
};
use compos_common::odrefresh::ExitCode;

const FD_SERVER_PORT: i32 = 3264; // TODO: support dynamic port

fn validate_args(args: &OdrefreshArgs) -> Result<()> {
    if args.compilationMode != CompilationMode::NORMAL_COMPILE {
        // Conservatively check debuggability.
        let debuggable =
            system_properties::read_bool("ro.boot.microdroid.debuggable", false).unwrap_or(false);
        if !debuggable {
            bail!("Requested compilation mode only available in debuggable VMs");
        }
    }

    if args.systemDirFd < 0 || args.outputDirFd < 0 || args.stagingDirFd < 0 {
        bail!("The remote FDs are expected to be non-negative");
    }
    if !matches!(&args.zygoteArch[..], "zygote64" | "zygote64_32") {
        bail!("Invalid zygote arch");
    }
    // Disallow any sort of path traversal
    if args.targetDirName.contains(path::MAIN_SEPARATOR) {
        bail!("Invalid target directory {}", args.targetDirName);
    }

    // We're not validating/allowlisting the compiler filter, and just assume the compiler will
    // reject an invalid string. We need to accept "verify" filter anyway, and potential
    // performance degration by the attacker is not currently in scope. This also allows ART to
    // specify new compiler filter and configure through system property without change to
    // CompOS.
    Ok(())
}

pub fn odrefresh<F>(
    odrefresh_path: &Path,
    args: &OdrefreshArgs,
    authfs_service: Strong<dyn IAuthFsService>,
    success_fn: F,
) -> Result<ExitCode>
where
    F: FnOnce(PathBuf) -> Result<()>,
{
    validate_args(args)?;

    // Mount authfs (via authfs_service). The authfs instance unmounts once the `authfs` variable
    // is out of scope.

    let mut input_dir_fd_annotations = vec![InputDirFdAnnotation {
        fd: args.systemDirFd,
        // Use the 0th APK of the extra_apks in compos/apk/assets/vm_config*.json
        manifestPath: "/mnt/extra-apk/0/assets/build_manifest.pb".to_string(),
        prefix: "system/".to_string(),
    }];
    if args.systemExtDirFd >= 0 {
        input_dir_fd_annotations.push(InputDirFdAnnotation {
            fd: args.systemExtDirFd,
            // Use the 1st APK of the extra_apks in compos/apk/assets/vm_config_system_ext_*.json
            manifestPath: "/mnt/extra-apk/1/assets/build_manifest.pb".to_string(),
            prefix: "system_ext/".to_string(),
        });
    }

    let authfs_config = AuthFsConfig {
        port: FD_SERVER_PORT,
        inputDirFdAnnotations: input_dir_fd_annotations,
        outputDirFdAnnotations: vec![
            OutputDirFdAnnotation { fd: args.outputDirFd },
            OutputDirFdAnnotation { fd: args.stagingDirFd },
        ],
        ..Default::default()
    };
    let authfs = authfs_service.mount(&authfs_config)?;
    let mountpoint = PathBuf::from(authfs.getMountPoint()?);

    // Make a copy of our environment as the basis of the one we will give odrefresh
    let mut odrefresh_vars = EnvMap::from_current_env();

    let mut android_root = mountpoint.clone();
    android_root.push(args.systemDirFd.to_string());
    android_root.push("system");
    odrefresh_vars.set("ANDROID_ROOT", path_to_str(&android_root)?);
    debug!("ANDROID_ROOT={:?}", &android_root);

    if args.systemExtDirFd >= 0 {
        let mut system_ext_root = mountpoint.clone();
        system_ext_root.push(args.systemExtDirFd.to_string());
        system_ext_root.push("system_ext");
        odrefresh_vars.set("SYSTEM_EXT_ROOT", path_to_str(&system_ext_root)?);
        debug!("SYSTEM_EXT_ROOT={:?}", &system_ext_root);
    }

    let art_apex_data = mountpoint.join(args.outputDirFd.to_string());
    odrefresh_vars.set("ART_APEX_DATA", path_to_str(&art_apex_data)?);
    debug!("ART_APEX_DATA={:?}", &art_apex_data);

    let staging_dir = mountpoint.join(args.stagingDirFd.to_string());

    set_classpaths(&mut odrefresh_vars, &android_root)?;

    let mut command_line_args = vec![
        "odrefresh".to_string(),
        "--compilation-os-mode".to_string(),
        format!("--zygote-arch={}", args.zygoteArch),
        format!("--dalvik-cache={}", args.targetDirName),
        format!("--staging-dir={}", staging_dir.display()),
        "--no-refresh".to_string(),
    ];

    if !args.systemServerCompilerFilter.is_empty() {
        command_line_args
            .push(format!("--system-server-compiler-filter={}", args.systemServerCompilerFilter));
    }

    let compile_flag = match args.compilationMode {
        CompilationMode::NORMAL_COMPILE => "--compile",
        CompilationMode::TEST_COMPILE => "--force-compile",
        other => bail!("Unknown compilation mode {:?}", other),
    };
    command_line_args.push(compile_flag.to_string());

    debug!("Running odrefresh with args: {:?}", &command_line_args);
    let jail = spawn_jailed_task(odrefresh_path, &command_line_args, &odrefresh_vars.into_env())
        .context("Spawn odrefresh")?;
    let exit_code = match jail.wait() {
        Ok(_) => 0,
        Err(minijail::Error::ReturnCode(exit_code)) => exit_code,
        Err(e) => bail!("Unexpected minijail error: {}", e),
    };

    let exit_code = ExitCode::from_i32(exit_code.into())?;
    info!("odrefresh exited with {:?}", exit_code);

    if exit_code == ExitCode::CompilationSuccess {
        let target_dir = art_apex_data.join(&args.targetDirName);
        success_fn(target_dir)?;
    }

    Ok(exit_code)
}

fn path_to_str(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| anyhow!("Bad path {:?}", path))
}

fn set_classpaths(odrefresh_vars: &mut EnvMap, android_root: &Path) -> Result<()> {
    let export_lines = run_derive_classpath(android_root)?;
    load_classpath_vars(odrefresh_vars, &export_lines)
}

fn run_derive_classpath(android_root: &Path) -> Result<String> {
    let classpaths_root = android_root.join("etc/classpaths");

    let mut bootclasspath_arg = OsString::new();
    bootclasspath_arg.push("--bootclasspath-fragment=");
    bootclasspath_arg.push(classpaths_root.join("bootclasspath.pb"));

    let mut systemserverclasspath_arg = OsString::new();
    systemserverclasspath_arg.push("--systemserverclasspath-fragment=");
    systemserverclasspath_arg.push(classpaths_root.join("systemserverclasspath.pb"));

    let result = Command::new("/apex/com.android.sdkext/bin/derive_classpath")
        .arg(bootclasspath_arg)
        .arg(systemserverclasspath_arg)
        .arg("/proc/self/fd/1")
        .output()
        .context("Failed to run derive_classpath")?;

    if !result.status.success() {
        bail!("derive_classpath returned {}", result.status);
    }

    String::from_utf8(result.stdout).context("Converting derive_classpath output")
}

fn load_classpath_vars(odrefresh_vars: &mut EnvMap, export_lines: &str) -> Result<()> {
    // Each line should be in the format "export <var name> <value>"
    let pattern = Regex::new(r"^export ([^ ]+) ([^ ]+)$").context("Failed to construct Regex")?;
    for line in export_lines.lines() {
        if let Some(captures) = pattern.captures(line) {
            let name = &captures[1];
            let value = &captures[2];
            odrefresh_vars.set(name, value);
        } else {
            warn!("Malformed line from derive_classpath: {}", line);
        }
    }

    Ok(())
}

fn spawn_jailed_task(executable: &Path, args: &[String], env_vars: &[String]) -> Result<Minijail> {
    // TODO(b/185175567): Run in a more restricted sandbox.
    let jail = Minijail::new()?;
    let keep_fds = [];
    let command = minijail::Command::new_for_path(executable, &keep_fds, args, Some(env_vars))?;
    let _pid = jail.run_command(command)?;
    Ok(jail)
}

struct EnvMap(HashMap<String, String>);

impl EnvMap {
    fn from_current_env() -> Self {
        Self(env::vars().collect())
    }

    fn set(&mut self, key: &str, value: &str) {
        self.0.insert(key.to_owned(), value.to_owned());
    }

    fn into_env(self) -> Vec<String> {
        // execve() expects an array of "k=v" strings, rather than a list of (k, v) pairs.
        self.0.into_iter().map(|(k, v)| k + "=" + &v).collect()
    }
}
