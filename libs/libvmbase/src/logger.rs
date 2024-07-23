// Copyright 2022, The Android Open Source Project
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

//! Logger for vmbase.
//!
//! Internally uses the println! vmbase macro, which prints to crosvm's UART.
//! Note: may not work if the VM is in an inconsistent state. Exception handlers
//! should avoid using this logger and instead print with eprintln!.

use crate::console::println;
use core::sync::atomic::{AtomicBool, Ordering};
use log::{Log, Metadata, Record, SetLoggerError};

struct Logger {
    is_enabled: AtomicBool,
}

static LOGGER: Logger = Logger::new();

impl Logger {
    const fn new() -> Self {
        Self { is_enabled: AtomicBool::new(true) }
    }

    fn swap_enabled(&self, enabled: bool) -> bool {
        self.is_enabled.swap(enabled, Ordering::Relaxed)
    }
}

impl Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        self.is_enabled.load(Ordering::Relaxed)
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            println!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

/// An RAII implementation of a log suppressor. When the instance is dropped, logging is re-enabled.
pub struct SuppressGuard {
    old_enabled: bool,
}

impl SuppressGuard {
    fn new() -> Self {
        Self { old_enabled: LOGGER.swap_enabled(false) }
    }
}

impl Drop for SuppressGuard {
    fn drop(&mut self) {
        LOGGER.swap_enabled(self.old_enabled);
    }
}

/// Initialize vmbase logger with a given max logging level.
pub(crate) fn init() -> Result<(), SetLoggerError> {
    log::set_logger(&LOGGER)?;
    Ok(())
}

/// Suppress logging until the return value goes out of scope.
pub fn suppress() -> SuppressGuard {
    SuppressGuard::new()
}
