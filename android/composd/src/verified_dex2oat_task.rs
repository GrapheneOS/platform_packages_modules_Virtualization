/*
 * Copyright 2025 The Android Open Source Project
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

//! Handle running dex2oat in the VM, with an async interface to allow cancellation
use crate::wrappers::compos_wrappers_injection;
use crate::{
    fd_server_helper::{FdServerConfig, FdWithFsvMeta},
    instance_manager::IInstanceManager,
    instance_starter::CompOsInstance,
    util,
    wrappers::compos_common_injection::{
        compos_client::{CompOsService, CompOsType},
        BUILD_MANIFEST_SYSTEM_EXT_APK_PATH,
    },
};
use compos_wrappers_injection::fsverity::{
    open_fsv_meta_from_target_fd, read_digest, read_digest_from_fsv_meta,
};

#[cfg(test)]
use compos_wrappers_injection::mock_paths as paths;
#[cfg(not(test))]
use compos_wrappers_injection::paths;

use android_system_composd::aidl::android::system::composd::ICompilationTask::BnCompilationTask;
use compos_aidl_interface::aidl::com::android::compos::{
    IVerifiedDex2OatService::{Dex2OatArg::Dex2OatArg as CompSvcArg, FileDetails::FileDetails},
    IVerifiedDex2OatTaskCallback::{
        BnVerifiedDex2OatTaskCallback, GuestDex2OatMetrics::GuestDex2OatMetrics,
        GuestFailureDetails::GuestFailureDetails, IVerifiedDex2OatTaskCallback,
    },
};

use android_system_composd::aidl::android::system::composd::{
    ICompilationTask::ICompilationTask,
    IDex2OatTaskCallback::{
        Dex2OatMetrics::Dex2OatMetrics, FailureDetails::FailureDetails,
        FailureReason::FailureReason, IDex2OatTaskCallback,
    },
    IIsolatedCompilationService::Dex2OatArg::Dex2OatArg,
};
use anyhow::{anyhow, Context, Result};
use binder::{BinderFeatures, Interface, ParcelFileDescriptor, Result as BinderResult, Strong};
use log::{debug, error, info, warn};
use nix::{fcntl, fcntl::OFlag};
use parking_lot::{Condvar, Mutex, WaitTimeoutResult};
use std::{
    ops::Add,
    os::fd::{AsFd, AsRawFd, RawFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
    time::{Duration, Instant},
};

#[cfg(not(test))]
use crate::fd_server_helper::FdServer;
#[cfg(test)]
use crate::fd_server_helper::MockFdServer as FdServer;

#[cfg(test)]
use crate::wrappers::binder::MockLazyServiceGuard as LazyServiceGuard;
#[cfg(not(test))]
use binder::LazyServiceGuard;

// A timeout meant to catch scenarios where a lock (that should be held for a very
// short period of time) took longer than expected.
const SHORT_TIMEOUT: Duration = Duration::from_millis(500);
// A very long timeout meant to catch scenarios where a lock took far longer
// than expected to be acquired.
const LONG_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_BASE_OS: &str = "microdroid";

struct WaitStateData {
    // Compilation arguments for verified dex2oat.
    args: Vec<CompSvcArg>,
    // Config needed to launch an instance of FdServer, owns all fds that will be exposed to the
    // VM.
    fd_server_config: FdServerConfig,
    // The /system directory. Actual fd is owned by FdConfig.
    system_dir_fd: RawFd,
    // The /system_ext directory if applicable, actual fd is owned by FdConfig. Set to -1
    // if the build manifest for /system_ext is not present.
    system_ext_dir_fd: RawFd,
    // The signed manifest CompSvc will write the compiler argument measurements into and sign.
    // The fd is owned by FdConfig.
    manifest_fd: RawFd,
}

struct RunStateData {
    compos_instance: CompOsInstance,
}

enum State {
    WAITING(WaitStateData),
    RUNNING(RunStateData),
    // Completed indicates that a compilation job finishes, either with
    // success or failure.
    COMPLETED,
    CANCELED,
}

impl PartialEq for State {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (State::WAITING(_), State::WAITING(_))
                | (State::RUNNING(_), State::RUNNING(_))
                | (State::COMPLETED, State::COMPLETED)
                | (State::CANCELED, State::CANCELED)
        )
    }
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state_string = match self {
            State::WAITING(_) => "WAITING",
            State::RUNNING(_) => "RUNNING",
            State::COMPLETED => "COMPLETED",
            State::CANCELED => "CANCELED",
        };
        write!(f, "{state_string}")
    }
}

#[derive(Clone)]
pub struct Dex2OatCancelTask {
    weak_job_state: Weak<Dex2OatJobState>,
}

impl Interface for Dex2OatCancelTask {}

impl ICompilationTask for Dex2OatCancelTask {
    fn cancel(&self) -> BinderResult<()> {
        let job_state = self.weak_job_state.upgrade();
        if let Some(state) = job_state {
            state.cancel_job();
        }
        // Job no longer running so we don't care.
        Ok(())
    }
}

impl Dex2OatCancelTask {
    fn new_binder(weak_job_state: Weak<Dex2OatJobState>) -> Strong<dyn ICompilationTask> {
        BnCompilationTask::new_binder(
            Dex2OatCancelTask { weak_job_state },
            binder::BinderFeatures::default(),
        )
    }
}

struct Dex2OatJobState {
    state: Mutex<Option<State>>,
    cond: Condvar,
    // The CompOSd service is a lazy service, once all references to it are
    // dropped it will normally exit.
    // This lazy service guard extends the lifetime of the service
    // to the lifetime of the compilation jobs.
    #[allow(dead_code)]
    lazy_service_guard: LazyServiceGuard,
}

impl Dex2OatJobState {
    fn new(state: State) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(Some(state)),
            cond: Condvar::new(),
            lazy_service_guard: LazyServiceGuard::new(),
        })
    }

    fn wait_for_finished_until(&self, timeout: Instant) -> WaitTimeoutResult {
        let mut state_guard = self.state.lock();
        self.cond.wait_while_until(
            &mut state_guard,
            |state| {
                let state_ref = state.as_ref().unwrap();
                *state_ref != State::CANCELED && *state_ref != State::COMPLETED
            },
            timeout,
        )
    }
    fn set_state_if<F>(&self, new_state: State, condition: F) -> Result<()>
    where
        F: Fn(&State) -> bool,
    {
        let mut guard = self.state.lock();

        if let Some(current_state) = guard.as_ref() {
            if condition(current_state) {
                *guard = Some(new_state);
                return Ok(());
            }
        }
        Err(anyhow!("State did not match expectations  {:?}", *guard))
    }

    fn cancel_job(&self) {
        let mut state_guard = self.state.lock();
        let state = state_guard.as_ref().unwrap();
        match state {
            State::COMPLETED | State::CANCELED => (),
            State::WAITING(_) | State::RUNNING(_) => {
                let old_state = state_guard.replace(State::CANCELED).unwrap();
                self.cond.notify_all();
                if let State::RUNNING(state_data) = old_state {
                    let _ = state_data.compos_instance.shutdown();
                }
            }
        }
    }
}

pub struct VerifiedDex2OatCompletionCallback {
    composd_completion_callback: Strong<dyn IDex2OatTaskCallback>,
    weak_job_state: Weak<Dex2OatJobState>,
}

impl Interface for VerifiedDex2OatCompletionCallback {}
impl IVerifiedDex2OatTaskCallback for VerifiedDex2OatCompletionCallback {
    fn onSuccess(&self, metrics: &GuestDex2OatMetrics) -> BinderResult<()> {
        if let Some(job_state) = self.weak_job_state.upgrade() {
            if let Err(e) =
                job_state.set_state_if(State::COMPLETED, |state| matches!(state, State::RUNNING(_)))
            {
                error!("Unexpected call to onSuccess: {:?}", e);
                return Ok(());
            }
            let cpu_time_milliseconds = metrics.cpu_time_milliseconds;
            let wallclock_time_milliseconds = metrics.wallclock_time_milliseconds;
            let out_metrics = Dex2OatMetrics { wallclock_time_milliseconds, cpu_time_milliseconds };
            return self.composd_completion_callback.onSuccess(&out_metrics);
        }
        warn!(
            "A completion for a verified dex2oat job was received after the dex2oat job
            state was destructed.  Likely a cancellation racing against a completion."
        );
        Ok(())
    }
    fn onFailure(&self, failure_details: &GuestFailureDetails) -> BinderResult<()> {
        error!("Compilation failed");
        if let Some(job_state) = self.weak_job_state.upgrade() {
            if let Err(e) =
                job_state.set_state_if(State::COMPLETED, |state| matches!(state, State::RUNNING(_)))
            {
                error!("Unexpected call to onFailure: {:?}", e);
                return Ok(());
            }

            let out_failure_details = match failure_details {
                GuestFailureDetails::Exit_code(f) => {
                    error!(
                        "dex2oat failed, exit code:{}, wallclock_time_ms: {}, cpu_time_ms: {}",
                        f.exit_code,
                        f.metrics.wallclock_time_milliseconds,
                        f.metrics.cpu_time_milliseconds
                    );
                    FailureDetails {
                        reason: FailureReason::Dex2OatFailed,
                        exit_code: f.exit_code,
                        signal: 0,
                        wallclock_time_milliseconds: f.metrics.wallclock_time_milliseconds,
                        cpu_time_milliseconds: f.metrics.cpu_time_milliseconds,
                        message: "".to_string(),
                    }
                }
                GuestFailureDetails::Signal(f) => {
                    error!("dex2oat failed due to signal, signal:{}, wallclock_time_ms: {}, cpu_time_ms: {}",
                    f.signal, f.metrics.wallclock_time_milliseconds, f.metrics.cpu_time_milliseconds);
                    FailureDetails {
                        reason: FailureReason::Dex2OatFailed,
                        exit_code: -1,
                        signal: f.signal,
                        wallclock_time_milliseconds: f.metrics.wallclock_time_milliseconds,
                        cpu_time_milliseconds: f.metrics.cpu_time_milliseconds,
                        message: "".to_string(),
                    }
                }
                GuestFailureDetails::Setup(f) => {
                    let fd_details: Vec<String> = f
                        .relevant_fds
                        .iter()
                        .map(|fd| format!("{}:{}", fd, util::get_path_from_fd(*fd).display()))
                        .collect();
                    let fd_details = fd_details.join(",");
                    error!("compilation setup failed:{} {}", f.message.clone(), fd_details);
                    FailureDetails {
                        reason: FailureReason::CompilationSetupFailed,
                        exit_code: -1,
                        signal: 0,
                        wallclock_time_milliseconds: -1,
                        cpu_time_milliseconds: -1,
                        message: f.message.clone(),
                    }
                }
            };
            return self.composd_completion_callback.onFailure(&out_failure_details);
        }
        warn!(
            "A completion for a verified dex2oat job was received after the dex2oat job
            state was destructed.  Likely a cancellation racing against a completion."
        );
        Ok(())
    }
}

struct Dex2OatJob {
    enqueued_at: Instant,
    timeout_at: Instant,
    job_state: Arc<Dex2OatJobState>,
    // The callback this service uses to communicate results to the client.
    composd_completion_callback: Strong<dyn IDex2OatTaskCallback>,
    // The callback the compsvc service uses to communicate results to this service.
    compsvc_completion_callback: Strong<dyn IVerifiedDex2OatTaskCallback>,
}
impl Dex2OatJob {
    pub fn new(
        state_data: WaitStateData,
        timeout: Duration,
        composd_completion_callback: &Strong<dyn IDex2OatTaskCallback>,
    ) -> Self {
        let job_state = Dex2OatJobState::new(State::WAITING(state_data));
        let timeout_at = Instant::now().add(timeout);
        let compsvc_completion_callback = BnVerifiedDex2OatTaskCallback::new_binder(
            VerifiedDex2OatCompletionCallback {
                composd_completion_callback: composd_completion_callback.clone(),
                weak_job_state: Arc::downgrade(&job_state),
            },
            BinderFeatures::default(),
        );
        Self {
            timeout_at,
            job_state,
            enqueued_at: Instant::now(),
            composd_completion_callback: composd_completion_callback.clone(),
            compsvc_completion_callback,
        }
    }

    // Try to read the fs-verity digest from fs-verity and if that fails fall back
    // to reading it from the appropriate fsv_meta file.
    fn start_job_and_wait_for_finish(&self, instance_manager: &dyn IInstanceManager) -> Result<()> {
        // Grab the state lock briefly to check if we're in the right state.
        {
            let state_guard = self.job_state.state.lock();
            let state = state_guard.as_ref().unwrap();
            match *state {
                State::COMPLETED | State::RUNNING(_) => {
                    return Err(anyhow!("Unexpected state {:?}", *state));
                }
                State::CANCELED => {
                    return Ok(());
                }
                State::WAITING(_) => (),
            }
        }
        // Starting up a VM takes a good amount of time so do all of this outside of the lock.
        let compos_instance = instance_manager
            .start_current_instance(CompOsType::Dex2Oat, DEFAULT_BASE_OS)
            .context("Unable to start VM")?;
        info!("VM started");
        let svc = match compos_instance.get_service() {
            CompOsService::Dex2Oat(svc) => Ok(svc),
            _ => Err(anyhow!(
                "The CompOS Instance unexpectedly returned an OdRefresh service
            instead of a Dex2Oat service"
            )),
        }?;
        info!("dex2oat service reference retrieved.");

        let mut state_guard = self.job_state.state.lock();
        let state_ref = state_guard.as_ref().ok_or(anyhow!(
            "Unable to start job: Job state is NONE, this should not be possible."
        ))?;

        match *state_ref {
            State::CANCELED => {
                info!("Job not started: job was canceled.");
                return Ok(());
            }
            State::COMPLETED | State::RUNNING(_) => {
                return Err(anyhow!("Unable to start job, unexpected state {:?}.", *state_ref));
            }
            State::WAITING(_) => {}
        }

        let start_time = Instant::now();
        if start_time > self.timeout_at {
            self.notify_job_timed_out(format!(
                "Job timed out before compilation started.
            Was enqueued at {:?} and spent {:?} in the queue.",
                self.enqueued_at,
                self.enqueued_at.elapsed()
            ));
            return Ok(());
        }
        let state = state_guard.take().unwrap();
        if let State::WAITING(state_data) = state {
            *state_guard = Some(State::RUNNING(RunStateData { compos_instance }));
            // Release the lock before calling any binder calls or waiting for the state.
            drop(state_guard);
            let _fd_server = FdServer::build_from_config(state_data.fd_server_config)
                .context("FdServer creation failed")?;

            info!("Reading system properties of host and setting system properties of CompOS");
            util::set_system_properties(|names, values| {
                svc.initializeSystemProperties(&names, &values)
                    .context("Initialize system properties")
            })?;
            info!("Starting verified dex2oat");
            svc.verifiedDex2Oat(
                &state_data.args,
                state_data.system_dir_fd,
                state_data.system_ext_dir_fd,
                state_data.manifest_fd.as_raw_fd(),
                &self.compsvc_completion_callback,
            )
            .context("Starting verified dex2oat failed")?;

            info!("dex2oat started, waiting for finish");
            // Wait for the job to finish, either by succeeding, failing or getting canceled.
            let result = self.job_state.wait_for_finished_until(self.timeout_at);
            if result.timed_out() {
                self.notify_job_timed_out(format!(
                    "compilation job enqueued at {:?} for {:?} ,
                    ran for {:?} before timing out at {:?}",
                    self.enqueued_at,
                    (start_time - self.enqueued_at),
                    start_time.elapsed(),
                    self.timeout_at
                ));
            }
            info!("Compilation finished");
            return Ok(());
        }
        unreachable!("while starting a compilation job state {:?} is not properly handled", state);
    }

    pub fn notify_job_timed_out(&self, message: String) {
        let failure_details = FailureDetails {
            reason: FailureReason::Timeout,
            exit_code: -1,
            wallclock_time_milliseconds: 0,
            cpu_time_milliseconds: 0,
            signal: 0,
            message,
        };
        error!("compilation timed out: {}", &failure_details.message);
        if let Err(e) = self.composd_completion_callback.onFailure(&failure_details) {
            error!("job timed out but unable to notify the client:{e}");
        }
    }

    pub fn notify_setup_failed(&self, message: String) {
        let failure_details = FailureDetails {
            reason: FailureReason::CompilationSetupFailed,
            exit_code: -1,
            wallclock_time_milliseconds: 0,
            cpu_time_milliseconds: 0,
            signal: 0,
            message: message.clone(),
        };
        if let Err(e) = self.composd_completion_callback.onFailure(&failure_details) {
            error!("setup failed but unable to notify the client:{e}, message={message}");
        }
    }
}

enum JobOrShutdown {
    JobAvailable(Dex2OatJob),
    Shutdown,
}

// A task queue allows for multiple compilation jobs to be started asynchronously.
// An example of this is an active foreground dex2oat compilation and a background dex2oat
// compilation. We intentionally do not launch verified dex2oat PVMs in parallel to avoid excessive
// consumption of memory and compute.
pub struct VerifiedDex2OatTaskQueue {
    queue: Mutex<Vec<Dex2OatJob>>,
    cond: Condvar,
    shutdown: AtomicBool,
}

impl VerifiedDex2OatTaskQueue {
    // Dequeue the next job or block and then dequeue when available.
    fn wait_for_shutdown_or_next_job(&self) -> Result<JobOrShutdown> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Ok(JobOrShutdown::Shutdown);
        }
        let mut queue_guard = self
            .queue
            .try_lock_for(LONG_TIMEOUT)
            .ok_or(anyhow!("Timed out acquiring mutex for the queue"))?;

        if self
            .cond
            .wait_while_for(&mut queue_guard, |queue| queue.is_empty(), LONG_TIMEOUT)
            .timed_out()
        {
            return Err(anyhow!("Timed out waiting for the queue to become non empty."));
        }
        if self.shutdown.load(Ordering::SeqCst) {
            return Ok(JobOrShutdown::Shutdown);
        }
        Ok(JobOrShutdown::JobAvailable(queue_guard.pop().unwrap()))
    }

    pub fn quit(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Acquire the lock to ensure we wake up any waiting threads.
        let _queue_guard = self.queue.lock();
        self.cond.notify_all();
    }

    pub fn enqueue_job(
        &self,
        dex2oat_args: &[Dex2OatArg],
        signed_manifest_fd: &ParcelFileDescriptor,
        timeout: Duration,
        completion_callback: &Strong<dyn IDex2OatTaskCallback>,
    ) -> Result<Strong<dyn ICompilationTask>> {
        let owned_manifest_fd = signed_manifest_fd
            .as_ref()
            .try_clone()
            .context("Failed to clone the signed manifest file descriptor")?;
        let system_dir_fd = util::open_dir(&paths::root_rebase("/system")).context(
            "Failed to generate compsvc args and start fdserver, unable to open /system",
        )?;

        let mut state_data = WaitStateData {
            args: Vec::<CompSvcArg>::new(),
            fd_server_config: FdServerConfig::default(),
            manifest_fd: owned_manifest_fd.as_raw_fd(),
            system_dir_fd: system_dir_fd.as_raw_fd(),
            system_ext_dir_fd: -1,
        };
        let args = &mut state_data.args;
        let fd_server_cfg = &mut state_data.fd_server_config;
        let ro_file_fds = &mut fd_server_cfg.ro_file_fds;
        let rw_file_fds = &mut fd_server_cfg.rw_file_fds;

        // There are two variants of a CompOS image, one that mounts the build manifest for the
        // system_ext partition and one that doesn't. Detect whether we should pass /system_ext fd
        // through by looking for the existence of the system ext manifest APK.
        let need_system_ext = paths::root_rebase(BUILD_MANIFEST_SYSTEM_EXT_APK_PATH).exists();
        (state_data.system_ext_dir_fd, fd_server_cfg.ro_dir_fds) = if need_system_ext {
            let system_ext_dir_fd = util::open_dir(paths::root_rebase("/system_ext").as_path())?;
            (system_ext_dir_fd.as_raw_fd(), vec![system_dir_fd, system_ext_dir_fd])
        } else {
            (-1, vec![system_dir_fd])
        };

        for dex2oat_arg in dex2oat_args.iter() {
            let mut arg =
                CompSvcArg { formatString: dex2oat_arg.formatString.clone(), fds: vec![] };
            for parcel_fd in dex2oat_arg.fds.iter() {
                let owned_fd = parcel_fd
                    .as_ref()
                    .try_clone()
                    .context("Cloning a parcel file descriptor failed")?;
                let fd = owned_fd.as_raw_fd();
                let fcntl_rval = fcntl::fcntl(fd, fcntl::F_GETFL)
                    .context("Unable to test if a fd is RW or RO, fcntl failed.")?;
                let access_mode =
                    OFlag::from_bits_truncate(fcntl_rval).intersection(OFlag::O_ACCMODE);
                let file_details = if access_mode == OFlag::O_RDONLY {
                    let borrowed_fd = owned_fd.as_fd();
                    let (verity_digest, fsv_meta_fd) = match read_digest(borrowed_fd) {
                        Ok(result) => (format!("sha256-{}", hex::encode(result)), None),
                        Err(_) => match read_digest_from_fsv_meta(borrowed_fd) {
                            Ok(fsv_digest_bytes) => (
                                format!("sha256-{}", hex::encode(fsv_digest_bytes)),
                                open_fsv_meta_from_target_fd(borrowed_fd).ok(),
                            ),
                            Err(e) => {
                                let file_path = util::get_path_from_fd(borrowed_fd.as_raw_fd());
                                debug!("No verity digest for {file_path:?} available: fallback to read from fsv_meta failed ({e})");
                                ("".to_string(), None)
                            }
                        },
                    };
                    ro_file_fds.push(FdWithFsvMeta { fd: owned_fd, fsv_meta_fd });
                    Ok(FileDetails { fd, isRw: false, verityDigest: verity_digest })
                } else if access_mode == OFlag::O_RDWR {
                    rw_file_fds.push(owned_fd);
                    Ok(FileDetails { fd, isRw: true, verityDigest: "".to_owned() })
                } else {
                    Err(anyhow!("A dex2oat arg fd has neither O_RDWR or O_RDONLY bits set"))
                }?;
                arg.fds.push(file_details);
            }
            args.push(arg);
        }
        rw_file_fds.push(owned_manifest_fd);

        let mut queue_guard = self.queue.try_lock_for(SHORT_TIMEOUT).ok_or(anyhow!("Timed out"))?;
        let new_job = Dex2OatJob::new(state_data, timeout, completion_callback);
        let result = Dex2OatCancelTask::new_binder(Arc::downgrade(&new_job.job_state));
        queue_guard.push(new_job);
        // The queue was empty before we added a job so notify listeners.
        if queue_guard.len() == 1 {
            self.cond.notify_all();
        }
        Ok(result)
    }

    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(vec![]),
            cond: Condvar::new(),
            shutdown: AtomicBool::new(false),
        })
    }
    // Start the dispatcher thread, reaper thread and return a reference to the job queue.
    pub fn start_job_dispatcher(queue: &Arc<Self>, instance_manager: Arc<dyn IInstanceManager>) {
        let job_queue = queue.clone();
        std::thread::spawn(move || {
            loop {
                let cur_job = match job_queue.wait_for_shutdown_or_next_job() {
                    Ok(JobOrShutdown::JobAvailable(job)) => job,
                    Ok(JobOrShutdown::Shutdown) => break, // Shutdown signal received
                    Err(e) => {
                        error!("Error waiting for job: {}", e);
                        continue; // Wait for the next job
                    }
                };
                if let Err(e) = cur_job.start_job_and_wait_for_finish(&*instance_manager) {
                    error!("Error starting job: {}", e);
                    cur_job
                        .notify_setup_failed(format!("Failed to start the compilation job: {}", e));
                }
                drop(cur_job);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fd_server_helper::MockFdServer,
        instance_manager::MockIInstanceManager,
        wrappers::{
            binder::MockLazyServiceGuard,
            compos_wrappers_injection::{fsverity, mock_paths, mock_system_properties},
        },
    };

    use android_system_composd::aidl::android::system::{composd::IDex2OatTaskCallback::{
        BnDex2OatTaskCallback, MockIDex2OatTaskCallback,
    }};
    use compos_aidl_interface::aidl::com::android::compos::IVerifiedDex2OatService::{
        BnVerifiedDex2OatService, MockIVerifiedDex2OatService,
        Dex2OatArg::Dex2OatArg as CompSvcArg,
        FileDetails::FileDetails
    };
    use compos_common_with_mocks::compos_client::MockComposClient;
    use std::{collections::{HashMap, HashSet}, fs::{create_dir, File}, io::Write, path::{Path, PathBuf}, os::fd::{BorrowedFd,OwnedFd}};
    use android_system_composd::aidl::android::system::composd::IIsolatedCompilationService::Dex2OatArg::Dex2OatArg;

    const ALLOWLISTED_PROPERTIES: [(&str, &str); 3] = [
        ("dalvik.vm.PROP1", "VAL1"),
        ("ro.dalvik.vm.PROP2", "VAL2"),
        ("persist.device_config.runtime_native_boot.PROP3", "VAL3"),
    ];
    // Properties that do not begin with allow-listed prefixes.
    const FILTERED_OUT_PROPERTIES: [(&str, &str); 3] =
        [("BAD_PROP1", "VAL1"), ("BAD_PROP2", "VAL2"), ("BAD_PROP3", "VAL3")];

    struct DoneBarrier {
        done_count: Mutex<u32>,
        cond_var: Condvar,
    }

    impl DoneBarrier {
        fn new(count: u32) -> Arc<Self> {
            Arc::new(Self { done_count: Mutex::<u32>::new(count), cond_var: Condvar::new() })
        }
        fn done(&self) {
            let mut guard = self.done_count.lock();
            *guard += 1;
            self.cond_var.notify_all();
        }

        fn wait_for_all_done_for(&self, timeout: Duration) {
            let mut guard = self.done_count.lock();
            self.cond_var.wait_while_for(&mut guard, |count| *count > 0, timeout);
        }
    }

    // Create a new parcel file descriptor initialized with some data and then re-opened as read
    // only
    fn new_ro_parcel_fd(
        dir: &Path,
        file_contents: String,
        enable_verity: bool,
    ) -> (bool, ParcelFileDescriptor, String) {
        let mut temp_file =
            tempfile::NamedTempFile::new_in(dir).expect("Unable to create a new file");
        temp_file
            .write_all(file_contents.as_bytes())
            .expect("Failed to write contents to temp file");
        let (_, pathbuf) = temp_file.keep().expect("Unable to keeping temp file");
        let file = File::open(pathbuf.as_path()).expect("Unable to open file as read only");

        let mut verity_digest = "".to_owned();
        if enable_verity {
            fsverity::enable(file.as_fd()).expect("Unable to enable fs-verity on a file");
            let digest_bytes = fsverity::read_digest(file.as_fd()).expect(
                "Error reading sha256-digest from file after enabling verity on the same file.",
            );
            verity_digest = format!("sha256-{}", hex::encode(digest_bytes));
        }
        let owned_fd: OwnedFd = file.into();
        (false, ParcelFileDescriptor::new(owned_fd), verity_digest)
    }

    // Create a new parcel file descriptor initialized with some data.
    fn new_rw_parcel_fd(dir: &Path, file_contents: String) -> (bool, ParcelFileDescriptor, String) {
        let mut temp_file =
            tempfile::NamedTempFile::new_in(dir).expect("Unable to create a new file");
        temp_file
            .write_all(file_contents.as_bytes())
            .expect("Failed to write contents to temp file");
        let (file, _) = temp_file.keep().expect("Problem with keeping a read write temp file");
        let owned_fd: OwnedFd = file.into();
        (true, ParcelFileDescriptor::new(owned_fd), "".to_owned())
    }
    // Take a fd and return the st_dev and st_ino.
    fn fd_to_st_dev_ino(fd: RawFd) -> (u64, u64) {
        let file_stat = nix::sys::stat::fstat(fd).expect("File stat failed");
        (file_stat.st_dev, file_stat.st_ino)
    }

    fn fd_are_same(fd1: RawFd, fd2: RawFd) -> bool {
        fd_to_st_dev_ino(fd1) == fd_to_st_dev_ino(fd2)
    }

    fn fds_are_the_same(fds1: &[RawFd], fds2: &[RawFd]) -> bool {
        fds1.iter().zip(fds2).all(|(fd1, fd2)| fd_are_same(*fd1, *fd2))
    }

    fn is_fd_writeable(fd: RawFd) -> Result<bool> {
        let fcntl_rval = fcntl::fcntl(fd, fcntl::F_GETFL)
            .context("Unable to test if a fd is RW or RO, fcntl failed.")?;
        let access_mode = OFlag::from_bits_truncate(fcntl_rval).intersection(OFlag::O_ACCMODE);
        Ok(access_mode == OFlag::O_RDWR)
    }

    fn get_fdserver_style_digest(fd: BorrowedFd) -> String {
        match fsverity::read_digest(fd) {
            Ok(digest_bytes) => format!("sha256-{}", hex::encode(digest_bytes)),
            Err(_) => "".to_string(),
        }
    }

    fn file_details_are_equiv(details_1: &[FileDetails], details_2: &[FileDetails]) -> bool {
        details_1.iter().zip(details_2).all(|(detail_1, detail_2)| {
            fd_are_same(detail_1.fd, detail_2.fd)
                && detail_1.isRw == detail_2.isRw
                && detail_1.verityDigest == detail_2.verityDigest
        })
    }

    fn compsvc_args_are_equiv(args1: &[CompSvcArg], args2: &[CompSvcArg]) -> bool {
        args1.iter().zip(args2).all(|(arg1, arg2)| file_details_are_equiv(&arg1.fds, &arg2.fds))
    }

    fn compsvc_args_from_dex2oat_args(args: &[Dex2OatArg]) -> Vec<CompSvcArg> {
        args.iter()
            .map(|arg| {
                let fds: Vec<FileDetails> = arg
                    .fds
                    .iter()
                    .map(|pfd| FileDetails {
                        fd: pfd.as_raw_fd(),
                        isRw: is_fd_writeable(pfd.as_raw_fd())
                            .expect("Failed to determine if fd is read-writeable"),
                        verityDigest: get_fdserver_style_digest(pfd.as_ref().as_fd()),
                    })
                    .collect();
                CompSvcArg { formatString: arg.formatString.clone(), fds }
            })
            .collect()
    }

    fn pop_first_char(in_str: &str) -> PathBuf {
        assert!(!in_str.is_empty());
        let mut in_string = in_str.to_owned();
        in_string.remove(0);
        PathBuf::from(in_string)
    }

    fn first_char_is(in_str: &str, match_char: char) -> bool {
        if let Some(first_char) = in_str.chars().next() {
            return first_char == match_char;
        }
        false
    }

    #[test]
    // Test for the correct job processing. Tests for the following:
    // args passed to VerifiedDex2oat are correct:
    //  - RO files with verity enabled contain the correct verity-digest
    //  - fds are in the same order and correspond to the same file as the files in Dex2OatArg.
    //  - The format string is exactly the same as the format string in Dex2OatArg.
    //  - systemfd corresponds to the correct directory
    // FdConfig is checked to make sure that the fds are in the right config list
    // Makes sure that LazyServiceGuard is created correctly and dropped correctly.
    fn new_jobs_processed() {
        // Create a fake manifest fd
        let temp_dir = tempfile::tempdir().expect("Unable to create a temp-dir");
        let temp_path = temp_dir.path();
        let manifest_parcel_fd = new_rw_parcel_fd(temp_path, "the manifest".to_owned());
        let system_path_buf = temp_path.join("system");
        create_dir(system_path_buf.as_path()).expect("Error creating system directory");
        let system_dir_fd =
            util::open_dir(system_path_buf.as_path()).expect("Problem opening system dir");
        // Mock paths to avoid filesystem errors
        let paths_ctx = mock_paths::root_rebase_context();
        let temp_pathbuf = temp_dir.path().to_path_buf();
        paths_ctx.expect().returning(move |sys_dir| {
            let mut rebased_dir = temp_pathbuf.clone();
            rebased_dir.push(pop_first_char(sys_dir));
            rebased_dir
        });
        let parcel_fds = vec![
            new_ro_parcel_fd(temp_path, "ro_file with verity".to_owned(), true),
            new_ro_parcel_fd(temp_path, "ro file w/o verity".to_owned(), false),
            new_rw_parcel_fd(temp_path, "rw file".to_owned()),
        ];
        // These are used to check the fdserver config and order within each fd list
        // doesn't matter. We store st_dev and st_ino because the code under test
        // will clone the fds so we need to compare using inode and dev.
        let mut expected_ro_dev_inos = HashSet::<(u64, u64)>::new();
        let mut expected_rw_dev_inos = HashSet::<(u64, u64)>::new();
        let mut expected_ro_dir_dev_inos = HashSet::<(u64, u64)>::new();
        expected_rw_dev_inos.insert(fd_to_st_dev_ino(manifest_parcel_fd.1.as_raw_fd()));
        expected_ro_dir_dev_inos.insert(fd_to_st_dev_ino(system_dir_fd.as_raw_fd()));

        let mut dex2oat_arg = Dex2OatArg {
            formatString: format!("compilerArg{}", ":!".repeat(parcel_fds.len())),
            fds: vec![],
        };

        let mut compsvc_arg =
            CompSvcArg { formatString: dex2oat_arg.formatString.clone(), fds: vec![] };

        // Prepare the expected raw fd lists for fd config
        for (is_rw, pfd, digest) in parcel_fds.into_iter() {
            let raw_fd = pfd.as_raw_fd();
            let st_dev_ino = fd_to_st_dev_ino(raw_fd);
            if is_rw {
                expected_rw_dev_inos.insert(st_dev_ino);
            } else {
                expected_ro_dev_inos.insert(st_dev_ino);
            }
            compsvc_arg.fds.push(FileDetails { fd: raw_fd, isRw: is_rw, verityDigest: digest });
            dex2oat_arg.fds.push(pfd);
        }

        let mock_fd_server_ctx = MockFdServer::build_from_config_context();
        let mut mock_fd_server = MockFdServer::default();
        mock_fd_server.expect_drop().times(1).return_const(());
        let holder: Arc<Mutex<Option<FdServerConfig>>> = Arc::new(Mutex::new(None));
        let holder_clone: Arc<Mutex<Option<FdServerConfig>>> = holder.clone();
        mock_fd_server_ctx
            .expect()
            .withf(move |fd_cfg| {
                let actual_ro_dev_inos = fd_cfg
                    .ro_file_fds
                    .iter()
                    .map(|fd| fd_to_st_dev_ino(fd.fd.as_raw_fd()))
                    .collect();
                let actual_rw_dev_inos =
                    fd_cfg.rw_file_fds.iter().map(|fd| fd_to_st_dev_ino(fd.as_raw_fd())).collect();
                let actual_ro_dir_dev_inos =
                    fd_cfg.ro_dir_fds.iter().map(|fd| fd_to_st_dev_ino(fd.as_raw_fd())).collect();
                let rw_dir_empty = fd_cfg.rw_dir_fds.is_empty();
                expected_ro_dev_inos == actual_ro_dev_inos
                    && expected_rw_dev_inos == actual_rw_dev_inos
                    && expected_ro_dir_dev_inos == actual_ro_dir_dev_inos
                    && rw_dir_empty
            })
            .return_once(move |fserver_config| {
                // prevent the file server config from eing destructed and taking its fds wth it.
                let mut guard = holder_clone.lock();
                *guard = Some(fserver_config);
                Ok(mock_fd_server)
            });
        let mut mock_guard = MockLazyServiceGuard::default();
        mock_guard.expect_drop().once().return_once(|| ());
        let mock_guard_new_context = MockLazyServiceGuard::new_context();
        mock_guard_new_context.expect().return_once(move || mock_guard);
        let mut mock_instance_manager = MockIInstanceManager::new();

        // Expectations for reading system properties and then initializing system
        // properties within the PVM.
        let system_properties_for_each_ctx = mock_system_properties::foreach_context();

        system_properties_for_each_ctx.expect().returning(
            |mut closure: Box<dyn for<'a, 'b> FnMut(&'a str, &'b str)>| {
                // Properties that begin with allow listed prefixes.
                ALLOWLISTED_PROPERTIES.iter().for_each(|(k, v)| closure(k, v));
                // Properties that do not begin with allow listed prefixes.
                FILTERED_OUT_PROPERTIES.iter().for_each(|(k, v)| closure(k, v));
                Ok(())
            },
        );
        // Expect system properties initialization
        let mut mock_service = MockIVerifiedDex2OatService::new();
        mock_service
            .expect_initializeSystemProperties()
            .withf(|k, v| {
                let in_set: HashMap<String, String> =
                    k.iter().zip(v.iter()).map(|(k, v)| (k.to_string(), v.to_string())).collect();
                let expected_set: HashMap<String, String> = ALLOWLISTED_PROPERTIES
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                in_set == expected_set
            })
            .return_once(|_, _| Ok(()));

        let dex2oat_args = vec![dex2oat_arg];
        let expect_compsvc_args = compsvc_args_from_dex2oat_args(&dex2oat_args);
        let expected_manifest_raw_fd = manifest_parcel_fd.1.as_raw_fd();
        // Expect verifiedDex2Oat call
        mock_service
            .expect_verifiedDex2Oat()
            .withf(move |compsvc_args, system_raw_fd, system_ext_raw_fd, manifest_raw_fd, _| {
                let args_match = compsvc_args_are_equiv(compsvc_args, &expect_compsvc_args);
                let manifest_match = fd_are_same(*manifest_raw_fd, expected_manifest_raw_fd);
                let system_dir_match = fd_are_same(*system_raw_fd, system_dir_fd.as_raw_fd());
                args_match && manifest_match && system_dir_match && *system_ext_raw_fd == -1
            })
            .return_once(|_, _, _, _, callback| {
                // Simulate success callback
                let metrics = GuestDex2OatMetrics {
                    cpu_time_milliseconds: 100,
                    wallclock_time_milliseconds: 200,
                };
                callback.onSuccess(&metrics).unwrap();
                Ok(())
            });

        let service_binder =
            BnVerifiedDex2OatService::new_binder(mock_service, BinderFeatures::default());
        let service_enum = CompOsService::Dex2Oat(service_binder);

        mock_instance_manager.expect_start_current_instance().return_once(move |_, _| {
            let vm_instance = MockComposClient::default();
            let mut lazy_service_guard = MockLazyServiceGuard::default();
            lazy_service_guard.expect_drop().return_const(());

            Ok(CompOsInstance::new_for_test(vm_instance, service_enum, lazy_service_guard))
        });

        let arc_instance_manager = Arc::new(mock_instance_manager);
        let queue = VerifiedDex2OatTaskQueue::new();
        VerifiedDex2OatTaskQueue::start_job_dispatcher(&queue, arc_instance_manager);

        let done_barrier = DoneBarrier::new(1);
        let done_barrier_clone = done_barrier.clone();
        // Mock completion callback
        let mut mock_completion_cb = MockIDex2OatTaskCallback::new();
        mock_completion_cb.expect_onSuccess().return_once(move |_| {
            done_barrier_clone.done();
            Ok(())
        });
        let mock_completion_cb_bn =
            BnDex2OatTaskCallback::new_binder(mock_completion_cb, BinderFeatures::default());

        // Enqueue job
        let _task = queue
            .enqueue_job(
                &dex2oat_args,
                &(manifest_parcel_fd.1),
                Duration::from_secs(1),
                &mock_completion_cb_bn,
            )
            .unwrap();
        done_barrier.wait_for_all_done_for(Duration::from_millis(300));
    }
}
