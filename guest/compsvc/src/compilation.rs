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
#[cfg(not(test))]
use crate::wrappers::{command_line_helper::run_derive_classpath, AuthFsFactory};
#[cfg(test)]
use crate::wrappers::{
    mock_command_line_helper::run_derive_classpath, MockAuthFsFactory as AuthFsFactory,
};
use anyhow::{anyhow, bail, Context, Result};
use bssl_crypto::digest;
#[cfg(not(test))]
use compos_wrappers::{
    minijail::{CommandFactory as minijail_command_factory, Minijail},
    paths, process_utils, system_properties,
};
#[cfg(test)]
use compos_wrappers_with_mocks::{
    minijail::{MockCommandFactory as minijail_command_factory, MockMinijail as Minijail},
    mock_paths as paths, mock_process_utils as process_utils,
    mock_system_properties as system_properties,
};
use log::{debug, error, info, warn};
use regex::Regex;
use rustix::param::clock_ticks_per_second;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::os::unix::raw::pid_t;
use std::path::{self, Path, PathBuf};
use std::thread;
use std::time::Instant;

use compos_common::odrefresh::ExitCode;

#[cfg(test)]
use crate::compos_key::mock_wrapper as compos_key;
#[cfg(not(test))]
use crate::compos_key::wrapper as compos_key;
#[cfg(test)]
use crate::fsverity::mock_wrapper as fsverity;
#[cfg(not(test))]
use crate::fsverity::wrapper as fsverity;
use authfs_aidl_interface::aidl::com::android::virt::fs::{
    AuthFsConfig::{
        AuthFsConfig, InputDirFdAnnotation::InputDirFdAnnotation,
        InputFdAnnotation::InputFdAnnotation, OutputDirFdAnnotation::OutputDirFdAnnotation,
        OutputFdAnnotation::OutputFdAnnotation,
        VerifiedInputFdAnnotation::VerifiedInputFdAnnotation,
    },
    IAuthFsService::IAuthFsService,
};
use compos_aidl_interface::aidl::com::android::compos::ICompOsService::{
    CompilationMode::CompilationMode, OdrefreshArgs::OdrefreshArgs,
};
use compos_aidl_interface::aidl::com::android::compos::IVerifiedDex2OatService::Dex2OatArg::Dex2OatArg;
use compos_aidl_interface::aidl::com::android::compos::IVerifiedDex2OatTaskCallback::{
    Dex2OatExitCode::Dex2OatExitCode, Dex2OatSetupFailure::Dex2OatSetupFailure,
    Dex2OatSignal::Dex2OatSignal, GuestDex2OatMetrics::GuestDex2OatMetrics,
    GuestFailureDetails::GuestFailureDetails, IVerifiedDex2OatTaskCallback,
};
use compos_manifest_proto::manifest::{
    signature::signed_manifest::secure_compile_manifest::compiler_argument::FileDetails as ProtoFileDetails,
    signature::signed_manifest::secure_compile_manifest::CompilerArgument,
    signature::signed_manifest::SecureCompileManifest, signature::SignedManifest, Signature,
    SignatureAlgorithm,
};

use binder::Strong;
use protobuf::Message;

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

fn get_input_dir_fd_annotations(
    system_dir_fd: i32,
    system_ext_dir_fd: i32,
) -> Vec<InputDirFdAnnotation> {
    let mut input_dir_fd_annotations = vec![InputDirFdAnnotation {
        fd: system_dir_fd,
        // Use the 0th APK of the extra_apks in compos/apk/assets/vm_config*.json
        manifestPath: "/mnt/extra-apk/0/assets/build_manifest.pb".to_string(),
        prefix: "system/".to_string(),
    }];
    if system_ext_dir_fd >= 0 {
        input_dir_fd_annotations.push(InputDirFdAnnotation {
            fd: system_ext_dir_fd,
            // Use the 1st APK of the extra_apks in
            // compos/apk/assets/vm_config_system_ext_*.json
            manifestPath: "/mnt/extra-apk/1/assets/build_manifest.pb".to_string(),
            prefix: "system_ext/".to_string(),
        });
    }
    input_dir_fd_annotations
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

    let input_dir_fd_annotations =
        get_input_dir_fd_annotations(args.systemDirFd, args.systemExtDirFd);
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
    info!("odrefresh exited with {exit_code:?}");

    if exit_code == ExitCode::CompilationSuccess {
        let target_dir = art_apex_data.join(&args.targetDirName);
        success_fn(target_dir)?;
    }

    Ok(exit_code)
}

fn path_to_str(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| anyhow!("Bad path {:?}", path))
}

fn set_classpaths(env_map: &mut EnvMap, android_root: &Path) -> Result<()> {
    let export_lines = run_derive_classpath(android_root)?;
    // Each line should be in the format "export <var name> <value>"
    let pattern = Regex::new(r"^export ([^ ]+) ([^ ]+)$").context("Failed to construct Regex")?;
    for line in export_lines.lines() {
        if let Some(captures) = pattern.captures(line) {
            let name = &captures[1];
            let value = &captures[2];
            env_map.set(name, value);
        } else {
            warn!("Malformed line from derive_classpath: {line}");
        }
    }
    Ok(())
}

fn spawn_jailed_task(
    executable: &Path,
    args: &Vec<String>,
    env_vars: &Vec<String>,
) -> Result<Minijail> {
    // TODO(b/185175567): Run in a more restricted sandbox.
    let jail = Minijail::new()?;
    let keep_fds = vec![];
    let command = minijail_command_factory::new_for_path(executable, &keep_fds, args, env_vars)?;
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

fn report_setup_failure(
    callback: &Strong<dyn IVerifiedDex2OatTaskCallback>,
    message: String,
    relevant_fds: Vec<i32>,
) {
    error!("setup failure: {message}, attempting to report failure to client.");
    let failure = GuestFailureDetails::Setup(Dex2OatSetupFailure { message, relevant_fds });
    if let Err(e) = callback.onFailure(&failure) {
        error!("Failed to report failure: {:?}", e);
    }
}

#[derive(Debug)]
struct FdError {
    pub message: String,
    pub fds: Vec<RawFd>,
}
impl std::fmt::Display for FdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}, fds:{:?}", self.message, self.fds)
    }
}
impl std::error::Error for FdError {}

#[derive(Debug)]
struct OpenedFds {
    pub fd: OwnedFd,
    pub is_rw: bool,
}
#[derive(Debug)]
struct ManifestCompilerArg {
    pub cmdline_arg: String,   // dex2oat cmdline arg
    pub arg: CompilerArgument, // CompilerArgument proto
    pub fds: Vec<OpenedFds>,   // Details about the open fds associated with this arg.
}

fn populate_cmdline_arg_with_fds(
    dex2oat_arg: &Dex2OatArg,
    opened_fds: &[OpenedFds],
) -> Result<String> {
    let mut fds_iter = opened_fds.iter();
    let mut cmdline_arg: String = String::new();
    let mut is_escaped = false;

    for cur_char in dex2oat_arg.formatString.chars() {
        if is_escaped {
            match cur_char {
                '\\' | '!' => {
                    cmdline_arg.push(cur_char);
                }
                _ => {
                    cmdline_arg.push('\\');
                    cmdline_arg.push(cur_char);
                }
            }
            is_escaped = false;
        } else if cur_char == '\\' {
            is_escaped = true;
        } else if cur_char == '!' {
            if let Some(compiled_fd) = fds_iter.next() {
                cmdline_arg.push_str(&compiled_fd.fd.as_raw_fd().to_string());
            } else {
                return Err(anyhow!(
                    "Mismatch between count of placeholders (!) and fds:formatString={}",
                    dex2oat_arg.formatString
                ));
            }
        } else {
            cmdline_arg.push(cur_char);
        }
    }
    if is_escaped {
        cmdline_arg.push('\\');
    }
    Ok(cmdline_arg)
}

fn prepare_manifest_args(
    args: &[Dex2OatArg],
    mountpoint: &Path,
) -> Result<(Vec<ManifestCompilerArg>, Vec<RawFd>), FdError> {
    let mut manifest_args: Vec<ManifestCompilerArg> = Vec::new();
    let mut raw_keep_fds: Vec<RawFd> = Vec::new();

    for arg in args {
        let mut opened_fds: Vec<OpenedFds> = Vec::new();
        let mut proto_file_details = Vec::new();

        for file_detail in &arg.fds {
            let mut verity_digest: Option<String> = None;
            let path = mountpoint.join(file_detail.fd.to_string());
            // RW files will never have verity digests available.
            let open_result = if file_detail.isRw {
                OpenOptions::new().read(true).write(true).truncate(true).open(&path)
            } else {
                // RO files optionally have verity digests.
                if !file_detail.verityDigest.is_empty() {
                    verity_digest = Some(file_detail.verityDigest.clone());
                }
                OpenOptions::new().read(true).open(&path)
            };
            let file = match open_result {
                Ok(file) => file,
                Err(e) => {
                    return Err(FdError {
                        message: format!(
                            "failed to open {}, rw={}, os_error {}",
                            path.display(),
                            file_detail.isRw,
                            e
                        ),
                        fds: vec![file_detail.fd],
                    });
                }
            };

            raw_keep_fds.push(file.as_raw_fd());
            let owned_fd: OwnedFd = file.into();

            let mut proto_detail = ProtoFileDetails::new();
            proto_detail.verity_digest = verity_digest;
            proto_file_details.push(proto_detail);

            opened_fds.push(OpenedFds { fd: owned_fd, is_rw: file_detail.isRw });
        }

        let cmdline_arg = match populate_cmdline_arg_with_fds(arg, &opened_fds) {
            Ok(c) => c,
            Err(e) => {
                return Err(FdError {
                    message: format!(
                        "Failed to build dex2oat arg for format {}: {:#}",
                        arg.formatString, e
                    ),
                    fds: vec![],
                });
            }
        };

        let mut compiler_arg = CompilerArgument::new();
        compiler_arg.compiler_flag = Some(arg.formatString.clone());
        compiler_arg.file_info = proto_file_details;

        manifest_args.push(ManifestCompilerArg { cmdline_arg, arg: compiler_arg, fds: opened_fds });
    }

    Ok((manifest_args, raw_keep_fds))
}

// For a given pid open a pid fd and wait for it to become readable. This should happen when the
// process terminates but the process is not yet reaped.
// Read the cpu time and wallclock time from procfs of the terminated but unreaped process.
fn get_pid_cpu_time_ms(pid: i32) -> Result<i32> {
    process_utils::wait_for_process_terminated(pid)?;
    // parse procfs
    let stat_path = paths::root_rebase(&format!("/proc/{}/stat", pid));
    let stat_string = fs::read_to_string(&stat_path)
        .with_context(|| format!("unable to open {}", stat_path.display()))?;
    let closing_paren_pos = stat_string
        .find(')')
        .ok_or_else(|| anyhow!("error parsing {}: unable to find ')'", stat_path.display()))?;
    let stat_str = &stat_string[closing_paren_pos..];
    let mut iter = stat_str.split_whitespace().skip(12);
    let utime = iter
        .next()
        .ok_or_else(|| anyhow!("Unable to find utime field in proc/pid/stat"))?
        .parse::<u64>()?;
    let stime = iter
        .next()
        .ok_or_else(|| anyhow!("Unable to find stime field in proc/pid/stat"))?
        .parse::<u64>()?;
    let cutime = iter
        .next()
        .ok_or_else(|| anyhow!("Unable to find cutime field in proc/pid/stat"))?
        .parse::<u64>()?;
    let cstime = iter
        .next()
        .ok_or_else(|| anyhow!("Unable to find cstime field in proc/pid/stat"))?
        .parse::<u64>()?;
    let total_cpu_ticks = utime + stime + cutime + cstime;
    Ok((total_cpu_ticks * 1000 / clock_ticks_per_second()).try_into()?)
}

fn fill_rw_verity_digests(manifest_args: &mut Vec<ManifestCompilerArg>) -> Result<()> {
    for manifest_arg in manifest_args {
        if manifest_arg.fds.len() != manifest_arg.arg.file_info.len() {
            // This should never happen.
            // The number of fds should always match the number of file infos.
            bail!(
                "Mismatch between number of fds and file infos for compiler flag: {:?}",
                manifest_arg.arg.compiler_flag
            );
        }
        for (opened_fd_info, file_info) in
            manifest_arg.fds.iter().zip(manifest_arg.arg.file_info.iter_mut())
        {
            if opened_fd_info.is_rw {
                // The fd is rw, we measure the verity now after compilation has
                // finished.
                if let Ok(digest) = fsverity::measure(opened_fd_info.fd.as_fd()) {
                    file_info.verity_digest = Some(format!("sha256-{}", hex::encode(digest)));
                } else {
                    // This should never happen.
                    // Measuring the verity on a rw file should always succeed.
                    bail!(
                        "Failed to measure rw fd {} for compiler flag: {:?}",
                        opened_fd_info.fd.as_raw_fd(),
                        manifest_arg.arg.compiler_flag
                    );
                }
            }
        }
    }
    Ok(())
}

fn record_manifest(manifest_args: Vec<ManifestCompilerArg>, manifest_path: &Path) -> Result<()> {
    let mut secure_compile_manifest = SecureCompileManifest::new();
    // Compilation is done, record verity digests for all files.
    secure_compile_manifest.compiler_arguments.extend(manifest_args.into_iter().map(|m| m.arg));
    let secure_compile_manifest_sha256: [u8; 32] = {
        let secure_compile_manifest_bytes = match secure_compile_manifest.write_to_bytes() {
            Ok(b) => b,
            Err(e) => {
                bail!("Failed to serialize manifest: {:?}", e);
            }
        };
        digest::Sha256::hash(&secure_compile_manifest_bytes)
    };
    let bytes_to_sign =
        [compos_common::COMPOS_MANIFEST_MAGIC_PREFIX.as_bytes(), &secure_compile_manifest_sha256]
            .concat();
    let signature_bytes = match compos_key::sign(&bytes_to_sign) {
        Ok(b) => b,
        Err(e) => {
            bail!("Failed to sign manifest: {:?}", e);
        }
    };
    let mut signed_manifest = SignedManifest::new();
    signed_manifest.manifest = protobuf::MessageField::some(secure_compile_manifest);
    signed_manifest.signature = Some(signature_bytes);
    signed_manifest.algorithm = Some(protobuf::EnumOrUnknown::new(SignatureAlgorithm::ED25519));

    let mut signature = Signature::new();
    signature.set_compos_signed_manifest(signed_manifest);
    let bytes_to_write = match signature.write_to_bytes() {
        Ok(b) => b,
        Err(e) => {
            bail!("Failed to serialize signature: {:#}", e);
        }
    };

    let mut manifest_file = match OpenOptions::new().write(true).truncate(true).open(manifest_path)
    {
        Ok(f) => f,
        Err(e) => {
            bail!("Failed to open manifest ({:?}) for writing: {:?}", manifest_path, e);
        }
    };

    if let Err(e) = manifest_file.write_all(&bytes_to_write) {
        bail!("failed to write manifest to {:?}:{:?}", manifest_path, e);
    }
    Ok(())
}

pub fn run_dex2oat(
    args: &[Dex2OatArg],
    system_dir_fd: i32,
    system_ext_dir_fd: i32,
    manifest_fd: i32,
    callback: &Strong<dyn IVerifiedDex2OatTaskCallback>,
) -> Result<()> {
    let authfs_service: Strong<dyn IAuthFsService> = AuthFsFactory::new_authfs_service()?;
    let dex2oat_path = Path::new("/apex/com.android.art/bin/dex2oat64");
    let dex2oat_binder_args = args.to_vec();
    let callback = callback.clone();

    thread::spawn(move || {
        // Create authfs config.
        let mut authfs_cfg = AuthFsConfig {
            port: FD_SERVER_PORT,
            inputDirFdAnnotations: get_input_dir_fd_annotations(system_dir_fd, system_ext_dir_fd),
            inputFdAnnotations: vec![],
            verifiedInputFdAnnotations: vec![],
            outputFdAnnotations: vec![OutputFdAnnotation { fd: manifest_fd }],
            ..Default::default()
        };

        for dex2oat_binder_arg in &dex2oat_binder_args {
            for file_details in &dex2oat_binder_arg.fds {
                if file_details.isRw {
                    authfs_cfg.outputFdAnnotations.push(OutputFdAnnotation { fd: file_details.fd });
                } else if file_details.verityDigest.is_empty() {
                    authfs_cfg.inputFdAnnotations.push(InputFdAnnotation { fd: file_details.fd });
                } else {
                    authfs_cfg.verifiedInputFdAnnotations.push(VerifiedInputFdAnnotation {
                        fd: file_details.fd,
                        digest: file_details.verityDigest.clone(),
                    });
                    info!(
                        "fd {} mounted with digest {}",
                        file_details.fd,
                        file_details.verityDigest.clone()
                    );
                }
            }
        }

        let authfs = match authfs_service.mount(&authfs_cfg) {
            Ok(service) => service,
            Err(e) => {
                report_setup_failure(&callback, format!("Failed to mount authfs: {:#}", e), vec![]);
                return;
            }
        };

        let mountpoint = match authfs.getMountPoint() {
            Ok(mp) => PathBuf::from(mp),
            Err(e) => {
                report_setup_failure(
                    &callback,
                    format!("Failed to get authfs mountpoint: {:#}", e),
                    vec![],
                );
                return;
            }
        };
        // Make a copy of environment variables.
        let mut env_vars = EnvMap::from_current_env();
        let mut android_root = mountpoint.clone();
        android_root.push(system_dir_fd.to_string());
        android_root.push("system");
        if let Err(e) = path_to_str(&android_root).map(|s| env_vars.set("ANDROID_ROOT", s)) {
            report_setup_failure(
                &callback,
                format!("Bad Android Root path({}): {:#}", android_root.display(), e),
                vec![],
            );
            return;
        }

        if system_ext_dir_fd >= 0 {
            let mut system_ext_root = mountpoint.clone();
            system_ext_root.push(system_ext_dir_fd.to_string());
            system_ext_root.push("system_ext");
            if let Err(e) =
                path_to_str(&system_ext_root).map(|s| env_vars.set("SYSTEM_EXT_ROOT", s))
            {
                report_setup_failure(
                    &callback,
                    format!("Bad System Ext Root path({}): {:#}", system_ext_root.display(), e),
                    vec![],
                );
                return;
            }
        }

        if let Err(e) = set_classpaths(&mut env_vars, &android_root) {
            report_setup_failure(&callback, format!("Failed to set classpaths: {:#}", e), vec![]);
            return;
        }

        let mut cmdline_args: Vec<String> = vec!["dex2oat64".to_string()];

        let result = prepare_manifest_args(&dex2oat_binder_args, &mountpoint);
        if let Err(e) = result {
            report_setup_failure(
                &callback,
                format!("Failed to open fds or build args: {:#}", e),
                e.fds,
            );
            return;
        }
        let (mut manifest_args, raw_keep_fds) = result.unwrap();

        for arg in &manifest_args {
            cmdline_args.push(arg.cmdline_arg.clone());
        }

        info!(
            "Running dex2oat with cmd={:?} args: {:?}, kept_fds={:?}",
            &dex2oat_path, &cmdline_args, &raw_keep_fds
        );
        let start_time = Instant::now();
        let env_vars = env_vars.into_env();
        let (jail, raw_pid) = match spawn_jailed_task_with_fds(
            dex2oat_path,
            &cmdline_args,
            &env_vars,
            &raw_keep_fds,
        ) {
            Ok(j) => j,
            Err(e) => {
                let err_msg = format!("Failed to spawn dex2oat, path={}, args={:?}, env_vars={:?}, kept_fds={:?}: {:#}",
                                                   dex2oat_path.display(), cmdline_args, env_vars,
                                                   raw_keep_fds, e);
                report_setup_failure(&callback, err_msg, vec![]);
                return;
            }
        };
        // get_pid_cpu_time_ms waits for the process to finish but does not reap it.
        // it will read the cpu and wallclock time from its procfs and leave the
        // process in a zombie state.
        let cpu_time_milliseconds = match get_pid_cpu_time_ms(raw_pid) {
            Ok(cpu_time_ms) => cpu_time_ms,
            Err(e) => {
                error!("Unable to determine cpu time for dex2oat: {:?}", e);
                0
            }
        };

        // jail.wait() will reap the process. When this returns the procfs of the process
        // will be gone.
        let jail_result = jail.wait();
        let wallclock_time_milliseconds = start_time.elapsed().as_millis() as i32;
        let metrics = GuestDex2OatMetrics { wallclock_time_milliseconds, cpu_time_milliseconds };
        if let Err(e) = jail_result {
            let failure_details = match e {
                minijail::Error::ReturnCode(exit_code) => {
                    GuestFailureDetails::Exit_code(Dex2OatExitCode {
                        exit_code: exit_code.into(),
                        metrics,
                    })
                }
                minijail::Error::Killed(signal) => {
                    GuestFailureDetails::Signal(Dex2OatSignal { signal: signal.into(), metrics })
                }
                _ => GuestFailureDetails::Setup(Dex2OatSetupFailure {
                    message: format!("dex2oat failed: {}", e),
                    relevant_fds: vec![],
                }),
            };
            let _ = callback.onFailure(&failure_details);
            error!("dex2oat failed: {failure_details:?}");
            return;
        }

        let manifest_path = mountpoint.join(manifest_fd.to_string());
        if let Err(e) = fill_rw_verity_digests(&mut manifest_args) {
            error!("Failed to fill the verity digests of rw files {:?}", e);
        }
        if let Err(e) = record_manifest(manifest_args, &manifest_path) {
            error!("Failed to record manifest: {:?}", e);
        }

        if let Err(e) = callback.onSuccess(&metrics) {
            error!("Failed to report success: {:?}", e);
        }
        info!(
            "dex2oat successfully completed: wallclock_time_ms={}, cpu_time_ms={}",
            wallclock_time_milliseconds, cpu_time_milliseconds
        );
    });

    Ok(())
}

fn spawn_jailed_task_with_fds(
    executable: &Path,
    args: &Vec<String>,
    env_vars: &Vec<String>,
    keep_fds: &Vec<RawFd>,
) -> Result<(Minijail, pid_t)> {
    // TODO(b/185175567): Run in a more restricted sandbox.
    let jail = Minijail::new()?;
    let command = minijail_command_factory::new_for_path(executable, keep_fds, args, env_vars)?;
    let pid = jail.run_command(command)?;
    Ok((jail, pid))
}

#[cfg(test)]
mod test {
    use super::*;
    use authfs_aidl_interface::aidl::com::android::virt::fs::{
        IAuthFs::BnAuthFs, IAuthFs::MockIAuthFs, IAuthFsService::BnAuthFsService,
        IAuthFsService::MockIAuthFsService,
    };
    use binder::{BinderFeatures, Strong};
    use compos_aidl_interface::aidl::com::android::compos::IVerifiedDex2OatService::FileDetails::FileDetails;
    use compos_aidl_interface::aidl::com::android::compos::IVerifiedDex2OatTaskCallback::{
        BnVerifiedDex2OatTaskCallback, IVerifiedDex2OatTaskCallback,
        MockIVerifiedDex2OatTaskCallback,
    };
    use compos_wrappers_with_mocks::minijail::{
        Command as mock_minijail_command, MockCommandFactory as mock_minijail_command_factory,
        MockMinijail,
    };
    use mockall::predicate::eq;
    use std::collections::HashSet;
    use std::fs;
    use std::os::fd::OwnedFd;
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::tempdir;

    // Creates a temporary file in a given directory, writes the given string to it
    // and depending on whether rw is true returns an OwnedFd with rw or an OwnedFd with ro
    // to it.
    fn create_test_file(dir: &Path, content: &str, rw: bool) -> OwnedFd {
        let path = dir.join(format!("test_file_{}", content.len()));
        fs::write(&path, content).expect("Failed to write test file");
        let mut options = OpenOptions::new();
        options.read(true);
        if rw {
            options.write(true);
        }
        options.open(&path).expect("Failed to open test file").into()
    }

    fn create_mock_owned_fd() -> OwnedFd {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        create_test_file(temp_dir.path(), "test", false)
    }

    #[test]
    fn test_populate_cmdline_arg_with_fds_simple_substitution() {
        let arg = Dex2OatArg { formatString: "--input-fd=!".to_string(), fds: vec![] };
        let fd1 = create_mock_owned_fd();
        let fd1_raw = fd1.as_raw_fd();
        let owned_fds = vec![OpenedFds { fd: fd1, is_rw: false }];

        let result = populate_cmdline_arg_with_fds(&arg, &owned_fds).unwrap();
        assert_eq!(result, format!("--input-fd={}", fd1_raw));
    }

    #[test]
    fn test_populate_cmdline_arg_with_fds_multiple_substitutions() {
        let arg = Dex2OatArg { formatString: "--fds=!,!".to_string(), fds: vec![] };
        let fd1 = create_mock_owned_fd();
        let fd2 = create_mock_owned_fd();
        let fd1_raw = fd1.as_raw_fd();
        let fd2_raw = fd2.as_raw_fd();
        let owned_fds =
            vec![OpenedFds { fd: fd1, is_rw: false }, OpenedFds { fd: fd2, is_rw: false }];

        let result = populate_cmdline_arg_with_fds(&arg, &owned_fds).unwrap();
        assert_eq!(result, format!("--fds={},{}", fd1_raw, fd2_raw));
    }

    #[test]
    fn test_populate_cmdline_arg_with_fds_escaped_placeholder() {
        let arg = Dex2OatArg { formatString: r"--not-a-placeholder=\!".to_string(), fds: vec![] };
        let owned_fds = vec![];

        let result = populate_cmdline_arg_with_fds(&arg, &owned_fds).unwrap();
        assert_eq!(result, "--not-a-placeholder=!");
    }

    #[test]
    fn test_populate_cmdline_arg_with_fds_escaped_escape() {
        let arg = Dex2OatArg { formatString: r"--path=\\!".to_string(), fds: vec![] };
        let fd1 = create_mock_owned_fd();
        let fd1_raw = fd1.as_raw_fd();
        let owned_fds = vec![OpenedFds { fd: fd1, is_rw: false }];

        let result = populate_cmdline_arg_with_fds(&arg, &owned_fds).unwrap();
        // \\ -> \
        // ! -> fd1_raw
        assert_eq!(result, format!("--path=\\{}", fd1_raw));
    }

    #[test]
    fn test_populate_cmdline_arg_with_fds_other_escape() {
        let arg = Dex2OatArg { formatString: "--misc=\n".to_string(), fds: vec![] };
        let owned_fds = vec![];
        let result = populate_cmdline_arg_with_fds(&arg, &owned_fds).unwrap();
        assert_eq!(result, "--misc=\n");
    }

    #[test]
    fn test_populate_cmdline_arg_with_fds_mismatch_error() {
        let arg = Dex2OatArg { formatString: "--input-fd=!".to_string(), fds: vec![] };
        let owned_fds = vec![];

        let result = populate_cmdline_arg_with_fds(&arg, &owned_fds);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Mismatch"));
    }

    #[test]
    fn test_prepare_manifest_args_success() {
        let temp_dir = tempdir().unwrap();
        let mountpoint = temp_dir.path();

        let fd1_path = mountpoint.join("10");
        let fd2_path = mountpoint.join("11");
        fs::write(&fd1_path, "ro data").unwrap();
        fs::write(&fd2_path, "rw data").unwrap();

        let dex2oat_args = vec![Dex2OatArg {
            formatString: "--fds=!,!".to_string(),
            fds: vec![
                FileDetails { fd: 10, isRw: false, verityDigest: "digest".to_string() },
                FileDetails { fd: 11, isRw: true, verityDigest: "".to_string() },
            ],
        }];

        let (manifest_args, raw_keep_fds) =
            prepare_manifest_args(&dex2oat_args, mountpoint).unwrap();

        assert_eq!(manifest_args.len(), 1);
        assert_eq!(manifest_args[0].fds.len(), 2);
        assert_eq!(raw_keep_fds.len(), 2);

        // Verify RO file
        assert_eq!(manifest_args[0].arg.file_info[0].verity_digest, Some("digest".to_string()));
        assert!(!manifest_args[0].fds[0].is_rw);

        // Verify RW file
        assert_eq!(manifest_args[0].arg.file_info[1].verity_digest, None);
        assert!(manifest_args[0].fds[1].is_rw);
    }

    #[test]
    fn test_prepare_manifest_args_missing_file() {
        let temp_dir = tempdir().unwrap();
        let mountpoint = temp_dir.path();

        let dex2oat_args = vec![Dex2OatArg {
            formatString: "--fds=!".to_string(),
            fds: vec![FileDetails { fd: 12, isRw: false, verityDigest: "".to_string() }],
        }];

        let result = prepare_manifest_args(&dex2oat_args, mountpoint);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("failed to open"));
        assert_eq!(err.fds, vec![12]);
    }

    #[test]
    fn test_validate_args_normal_compile_success() {
        let args = OdrefreshArgs {
            compilationMode: CompilationMode::NORMAL_COMPILE,
            systemDirFd: 1,
            outputDirFd: 2,
            stagingDirFd: 3,
            zygoteArch: "zygote64".to_string(),
            targetDirName: "target".to_string(),
            ..Default::default()
        };
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_test_compile_debuggable() {
        let ctx = system_properties::read_bool_context();
        ctx.expect().returning(|_, _| Ok(true));

        let args = OdrefreshArgs {
            compilationMode: CompilationMode::TEST_COMPILE,
            systemDirFd: 1,
            outputDirFd: 2,
            stagingDirFd: 3,
            zygoteArch: "zygote64".to_string(),
            targetDirName: "target".to_string(),
            ..Default::default()
        };
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_test_compile_not_debuggable() {
        let ctx = system_properties::read_bool_context();
        ctx.expect().returning(|_, _| Ok(false));

        let args = OdrefreshArgs {
            compilationMode: CompilationMode::TEST_COMPILE,
            systemDirFd: 1,
            outputDirFd: 2,
            stagingDirFd: 3,
            zygoteArch: "zygote64".to_string(),
            targetDirName: "target".to_string(),
            ..Default::default()
        };
        let result = validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("debuggable"));
    }

    #[test]
    fn test_run_dex2oat_success() {
        let (tx, rx) = mpsc::channel();
        let mut mock_callback = MockIVerifiedDex2OatTaskCallback::default();
        mock_callback.expect_onSuccess().times(1).returning(move |_| {
            tx.send(()).unwrap();
            Ok(())
        });
        let callback: Strong<dyn IVerifiedDex2OatTaskCallback> =
            BnVerifiedDex2OatTaskCallback::new_binder(mock_callback, BinderFeatures::default());

        let android_root = tempdir().unwrap();
        let mountpoint = android_root.path().join("authfs");
        fs::create_dir_all(&mountpoint).unwrap();

        let system_dir_fd = 10;
        let system_ext_dir_fd = 11;
        let manifest_fd = 20;

        let fd_ro_with_verity = 21;
        const FD_RO_DIGEST_SHA256: [u8; 32] = [0x55u8; 32];
        let fd_ro_with_verity_digest = format!("sha256-{}", hex::encode(FD_RO_DIGEST_SHA256));
        let fd_ro_no_verity = 22;
        let fd_rw = 23;

        let arg_test = Dex2OatArg { formatString: "--test-arg".to_string(), fds: vec![] };
        let arg_ro_no_verity = Dex2OatArg {
            formatString: "--zip-fd=!".to_string(),
            fds: vec![FileDetails {
                fd: fd_ro_no_verity,
                isRw: false,
                verityDigest: "".to_string(),
            }],
        };
        let arg_rw = Dex2OatArg {
            formatString: "--oat-fd=!".to_string(),
            fds: vec![FileDetails { fd: fd_rw, isRw: true, verityDigest: "".to_string() }],
        };
        let arg_ro_with_verity = Dex2OatArg {
            formatString: "--input-vdex-fd=!".to_string(),
            fds: vec![FileDetails {
                fd: fd_ro_with_verity,
                isRw: false,
                verityDigest: fd_ro_with_verity_digest.clone(),
            }],
        };

        let system_dir_path = mountpoint.join(system_dir_fd.to_string());
        let system_ext_dir_path = mountpoint.join(system_ext_dir_fd.to_string());
        let manifest_path = mountpoint.join(manifest_fd.to_string());

        fs::write(mountpoint.join(fd_ro_with_verity.to_string()), "ro_yes_verity").unwrap();
        fs::write(mountpoint.join(fd_ro_no_verity.to_string()), "ro_no_verity").unwrap();
        fs::File::create(mountpoint.join(fd_rw.to_string())).unwrap();
        fs::File::create(&manifest_path).unwrap();

        // Prepare mock directories
        fs::create_dir_all(system_dir_path.join("system/etc/classpaths")).unwrap();
        fs::create_dir_all(system_ext_dir_path.join("system_ext/etc/classpaths")).unwrap();

        let _authfs_factory_ctx = {
            let mut authfs_svc = MockIAuthFsService::default();
            let mut authfs = MockIAuthFs::default();
            let mp = mountpoint.to_str().unwrap().to_string();
            authfs.expect_getMountPoint().returning(move || Ok(mp.clone()));
            let fd_ro_with_verity_digest_clone = fd_ro_with_verity_digest.clone();
            authfs_svc
                .expect_mount()
                .withf(move |cfg| {
                    let input_dir_fds: HashSet<_> =
                        cfg.inputDirFdAnnotations.iter().map(|a| a.fd).collect();
                    let input_fds: HashSet<_> =
                        cfg.inputFdAnnotations.iter().map(|a| a.fd).collect();
                    let verified_input_fds: HashSet<_> = cfg
                        .verifiedInputFdAnnotations
                        .iter()
                        .map(|a| (a.fd, a.digest.clone()))
                        .collect();
                    let output_fds: HashSet<_> =
                        cfg.outputFdAnnotations.iter().map(|a| a.fd).collect();

                    cfg.port == FD_SERVER_PORT
                        && input_dir_fds == HashSet::from([system_dir_fd, system_ext_dir_fd])
                        && input_fds == HashSet::from([fd_ro_no_verity])
                        && verified_input_fds
                            == HashSet::from([(
                                fd_ro_with_verity,
                                fd_ro_with_verity_digest_clone.clone(),
                            )])
                        && output_fds == HashSet::from([manifest_fd, fd_rw])
                })
                .return_once(move |_| Ok(BnAuthFs::new_binder(authfs, BinderFeatures::default())));
            let ctx = AuthFsFactory::new_authfs_service_context();
            ctx.expect().return_once(move || {
                Ok(BnAuthFsService::new_binder(authfs_svc, BinderFeatures::default()))
            });
            ctx
        };

        let _derive_cp_ctx = {
            let ctx = crate::wrappers::mock_command_line_helper::run_derive_classpath_context();
            ctx.expect().returning(|_| Ok("".to_string()));
            ctx
        };

        let signature_bytes =
            b"this is a sixty four byte signature used for testing compos logic".to_vec();
        let signature_bytes_clone = signature_bytes.clone();
        let signature_bytes_for_assert = signature_bytes.clone();
        let _sign_ctx = {
            let ctx = crate::compos_key::mock_wrapper::sign_context();
            ctx.expect().return_once(move |_| Ok(signature_bytes_clone));
            ctx
        };

        let _fsverity_ctx = {
            let ctx = fsverity::measure_context();
            // We expect exactly one call for the one file with a verity digest
            ctx.expect().once().returning(|_| Ok(FD_RO_DIGEST_SHA256));
            ctx
        };

        let mock_minijail_command_tag: u32 = 54321;
        let _new_for_path_ctx = {
            let ctx = mock_minijail_command_factory::new_for_path_context();
            ctx.expect()
                .withf(move |_, keep_fds, args, _| {
                    let keep_fds_set: HashSet<_> = keep_fds.iter().cloned().collect();
                    keep_fds_set.len() == 3
                        && args[0] == "dex2oat64"
                        && args.iter().any(|a| a == "--test-arg")
                        && args.iter().any(|a| a.starts_with("--zip-fd="))
                        && args.iter().any(|a| a.starts_with("--oat-fd="))
                        && args.iter().any(|a| a.starts_with("--input-vdex-fd="))
                        && args.len() == 5
                })
                .return_once(move |_, _, _, _| {
                    Ok(mock_minijail_command { real_command: None, tag: mock_minijail_command_tag })
                });
            ctx
        };

        let dex2oat_pid: i32 = 5575;
        let dex2oat_procfs_path = android_root.path().join("proc").join(format!("{}", dex2oat_pid));
        fs::create_dir_all(&dex2oat_procfs_path).unwrap();
        let dex2oat_procfs_stat_path = dex2oat_procfs_path.join("stat");
        fs::write(
            dex2oat_procfs_stat_path.clone(),
            "12345 (test) S 1 1 1 0 -1 4194560 1 1 1 1 10 20 5 8 20 0 1 0 12345 123456789 1024 \
            18446744073709551615 1 1 0 0 0 0 0 4096 1 1 0 0 17 0 0 0 0 0 0 0 0 0 0 0 0 0 0",
        )
        .unwrap();
        let _root_rebase_ctx = {
            let ctx = paths::root_rebase_context();
            let proc_stat_str = format!("/proc/{}/stat", dex2oat_pid);
            ctx.expect()
                .withf(move |s| s == proc_stat_str)
                .return_once(move |_| dex2oat_procfs_stat_path);
            ctx
        };
        let _mock_minijail_new_ctx = {
            let mut mock_jail = MockMinijail::default();
            mock_jail.expect_run_command().returning(move |_| Ok(dex2oat_pid));
            mock_jail.expect_wait().times(1).returning(|| Ok(()));
            let ctx = MockMinijail::new_context();
            ctx.expect().return_once(|| Ok(mock_jail));
            ctx
        };

        let _wait_for_process_terminated_ctx = {
            let ctx = process_utils::wait_for_process_terminated_context();
            ctx.expect().with(eq(dex2oat_pid)).returning(|_| Ok(()));
            ctx
        };

        let args = vec![arg_test, arg_ro_no_verity, arg_rw, arg_ro_with_verity];

        assert!(
            run_dex2oat(&args, system_dir_fd, system_ext_dir_fd, manifest_fd, &callback).is_ok()
        );

        rx.recv_timeout(Duration::from_secs(5)).expect("Callback timed out");

        // Now we should look into the protobuf associated with manifest_fd and verify that
        // the compiler arguments are correctly recorded. That the verity digests, if applicable,
        // are also correctly recorded and that the signature matches the expected signature.
        let manifest_bytes = fs::read(&manifest_path).expect("Failed to read manifest");
        let signature =
            Signature::parse_from_bytes(&manifest_bytes).expect("Failed to parse signature");
        let signed_manifest = signature.compos_signed_manifest();
        let manifest = signed_manifest.manifest.as_ref().expect("Missing manifest");

        assert_eq!(manifest.compiler_arguments.len(), args.len());

        // Check first arg: --test-arg, no fds
        assert_eq!(
            manifest.compiler_arguments[0].compiler_flag,
            Some(args[0].formatString.clone())
        );
        assert!(manifest.compiler_arguments[0].file_info.is_empty());

        // Check second arg: --zip-fd=!, one fd, no verity
        assert_eq!(
            manifest.compiler_arguments[1].compiler_flag,
            Some(args[1].formatString.clone())
        );
        assert_eq!(manifest.compiler_arguments[1].file_info.len(), 1);
        assert!(manifest.compiler_arguments[1].file_info[0].verity_digest.is_none());

        // Check third arg: --oat-fd=!, one fd, WITH verity (RW)
        assert_eq!(
            manifest.compiler_arguments[2].compiler_flag,
            Some(args[2].formatString.clone())
        );
        assert_eq!(manifest.compiler_arguments[2].file_info.len(), 1);
        assert_eq!(
            manifest.compiler_arguments[2].file_info[0].verity_digest,
            Some(format!("sha256-{}", hex::encode(FD_RO_DIGEST_SHA256)))
        );

        // Check fourth arg: --input-vdex-fd=!, one fd, WITH verity
        assert_eq!(
            manifest.compiler_arguments[3].compiler_flag,
            Some(args[3].formatString.clone())
        );
        assert_eq!(manifest.compiler_arguments[3].file_info.len(), 1);
        assert_eq!(
            manifest.compiler_arguments[3].file_info[0].verity_digest,
            Some(fd_ro_with_verity_digest.clone())
        );

        assert_eq!(signed_manifest.signature.as_ref().unwrap(), &signature_bytes_for_assert);
        assert_eq!(
            signed_manifest.algorithm.unwrap().enum_value().unwrap(),
            SignatureAlgorithm::ED25519
        );
    }

    #[test]
    fn test_run_dex2oat_mount_failure() {
        let (tx, rx) = mpsc::channel();
        let mut mock_callback = MockIVerifiedDex2OatTaskCallback::default();
        mock_callback.expect_onFailure().times(1).returning(move |details| {
            if let GuestFailureDetails::Setup(setup_failure) = details {
                if setup_failure.message.contains("Failed to mount authfs") {
                    tx.send(()).unwrap();
                }
            }
            Ok(())
        });
        let callback =
            BnVerifiedDex2OatTaskCallback::new_binder(mock_callback, BinderFeatures::default());

        let _authfs_factory_ctx = {
            let mut authfs_svc = MockIAuthFsService::default();
            authfs_svc
                .expect_mount()
                .return_once(move |_| Err(binder::Status::new_service_specific_error(1, None)));
            let ctx = AuthFsFactory::new_authfs_service_context();
            ctx.expect().return_once(move || {
                Ok(BnAuthFsService::new_binder(authfs_svc, BinderFeatures::default()))
            });
            ctx
        };

        let args = vec![Dex2OatArg { formatString: "--test-arg".to_string(), fds: vec![] }];
        run_dex2oat(&args, 1, -1, 4, &callback).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).expect("Callback timed out");
    }

    #[test]
    fn test_populate_cmdline_arg_with_fds_complex_string() {
        let arg =
            Dex2OatArg { formatString: r#"! --path=\! --another=!"#.to_string(), fds: vec![] };
        let fd1 = create_mock_owned_fd();
        let fd2 = create_mock_owned_fd();
        let fd1_raw = fd1.as_raw_fd();
        let fd2_raw = fd2.as_raw_fd();
        let owned_fds =
            vec![OpenedFds { fd: fd1, is_rw: false }, OpenedFds { fd: fd2, is_rw: false }];

        let result = populate_cmdline_arg_with_fds(&arg, &owned_fds).unwrap();
        assert_eq!(result, format!("{} --path=! --another={}", fd1_raw, fd2_raw));
    }

    #[test]
    fn test_populate_cmdline_arg_with_fds_doubly_escaped_placeholder() {
        let arg = Dex2OatArg { formatString: r#"--path=\\\!"#.to_string(), fds: vec![] };
        let owned_fds: Vec<OpenedFds> = vec![];

        let result = populate_cmdline_arg_with_fds(&arg, &owned_fds).unwrap();
        assert_eq!(result, r#"--path=\!"#);
    }

    #[test]
    fn test_record_manifest() {
        let temp_dir = tempdir().unwrap();
        let manifest_path = temp_dir.path().join("manifest.pb");
        fs::File::create(&manifest_path).unwrap();

        let mut compiler_arg1 = CompilerArgument::new();
        compiler_arg1.compiler_flag = Some("--flag1".to_string());
        let manifest_arg1 = ManifestCompilerArg {
            cmdline_arg: "cmd1".to_string(),
            arg: compiler_arg1,
            fds: vec![],
        };

        let mut compiler_arg2 = CompilerArgument::new();
        compiler_arg2.compiler_flag = Some("--flag2".to_string());
        let mut file_info1 = ProtoFileDetails::new();
        file_info1.verity_digest = Some("digest1".to_string());
        let mut file_info2 = ProtoFileDetails::new();
        file_info2.verity_digest = Some("digest2".to_string());
        compiler_arg2.file_info = vec![file_info1, file_info2];
        let manifest_arg2 = ManifestCompilerArg {
            cmdline_arg: "cmd2".to_string(),
            arg: compiler_arg2,
            fds: vec![],
        };

        let signature_bytes = b"fake_signature_bytes".to_vec();
        let signature_bytes_clone = signature_bytes.clone();
        let _sign_ctx = {
            let ctx = crate::compos_key::mock_wrapper::sign_context();
            ctx.expect().times(1).return_once(move |_| Ok(signature_bytes_clone));
            ctx
        };

        assert!(record_manifest(vec![manifest_arg1, manifest_arg2], &manifest_path).is_ok());

        let manifest_bytes = fs::read(&manifest_path).expect("Failed to read manifest");
        let signature =
            Signature::parse_from_bytes(&manifest_bytes).expect("Failed to parse signature");
        let signed_manifest = signature.compos_signed_manifest();

        assert_eq!(signed_manifest.signature.as_ref().unwrap(), &signature_bytes);
        assert_eq!(
            signed_manifest.algorithm.unwrap().enum_value().unwrap(),
            SignatureAlgorithm::ED25519
        );
        let manifest = signed_manifest.manifest.as_ref().expect("Missing manifest");
        assert_eq!(manifest.compiler_arguments.len(), 2);
        assert_eq!(manifest.compiler_arguments[0].compiler_flag.as_ref().unwrap(), "--flag1");
        assert!(manifest.compiler_arguments[0].file_info.is_empty());

        assert_eq!(manifest.compiler_arguments[1].compiler_flag.as_ref().unwrap(), "--flag2");
        assert_eq!(manifest.compiler_arguments[1].file_info.len(), 2);
        assert_eq!(
            manifest.compiler_arguments[1].file_info[0].verity_digest.as_ref().unwrap(),
            "digest1"
        );
        assert_eq!(
            manifest.compiler_arguments[1].file_info[1].verity_digest.as_ref().unwrap(),
            "digest2"
        );
    }

    #[test]
    fn test_fill_rw_verity_digests() {
        let fd_rw = create_mock_owned_fd();
        let fd_ro = create_mock_owned_fd();

        let fds = vec![OpenedFds { fd: fd_rw, is_rw: true }, OpenedFds { fd: fd_ro, is_rw: false }];

        let proto_detail_rw = ProtoFileDetails::new();
        let mut proto_detail_ro = ProtoFileDetails::new();
        proto_detail_ro.verity_digest = Some("original_ro_digest".to_string());

        let mut compiler_arg = CompilerArgument::new();
        compiler_arg.compiler_flag = Some("--test-flag".to_string());
        compiler_arg.file_info = vec![proto_detail_rw, proto_detail_ro];

        let manifest_arg =
            ManifestCompilerArg { cmdline_arg: "--test-flag".to_string(), arg: compiler_arg, fds };

        let mut manifest_args = vec![manifest_arg];

        let _fsverity_ctx = {
            let ctx = fsverity::measure_context();
            ctx.expect().once().returning(|_| Ok([0xaa; 32]));
            ctx
        };

        assert!(fill_rw_verity_digests(&mut manifest_args).is_ok());

        let rw_digest = &manifest_args[0].arg.file_info[0].verity_digest;
        assert_eq!(rw_digest.as_ref().unwrap(), &format!("sha256-{}", hex::encode([0xaa; 32])));

        let ro_digest = &manifest_args[0].arg.file_info[1].verity_digest;
        assert_eq!(ro_digest.as_ref().unwrap(), "original_ro_digest");
    }
}
