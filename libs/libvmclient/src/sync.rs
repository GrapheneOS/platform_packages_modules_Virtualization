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

use std::{
    sync::{Condvar, LockResult, Mutex, MutexGuard, PoisonError, WaitTimeoutResult},
    time::Duration,
};

/// A mutex with an associated condition variable.
#[derive(Debug)]
pub struct Monitor<T> {
    pub state: Mutex<T>,
    pub cv: Condvar,
}

impl<T> Monitor<T> {
    /// Creates a new mutex wrapping the given value, and a new condition variable to go with it.
    pub fn new(state: T) -> Self {
        Self { state: Mutex::new(state), cv: Condvar::default() }
    }

    /// Waits on the condition variable while the given condition holds true on the contents of the
    /// mutex.
    ///
    /// Blocks until the condition variable is notified and the function returns false.
    pub fn wait_while(&self, condition: impl FnMut(&mut T) -> bool) -> LockResult<MutexGuard<T>> {
        self.cv.wait_while(self.state.lock()?, condition)
    }

    /// Waits on the condition variable while the given condition holds true on the contents of the
    /// mutex, with a timeout.
    ///
    /// Blocks until the condition variable is notified and the function returns false, or the
    /// timeout elapses.
    pub fn wait_timeout_while(
        &self,
        timeout: Duration,
        condition: impl FnMut(&mut T) -> bool,
    ) -> Result<(MutexGuard<T>, WaitTimeoutResult), PoisonError<MutexGuard<T>>> {
        self.cv
            .wait_timeout_while(self.state.lock()?, timeout, condition)
            .map_err(convert_poison_error)
    }
}

fn convert_poison_error<T>(err: PoisonError<(T, WaitTimeoutResult)>) -> PoisonError<T> {
    PoisonError::new(err.into_inner().0)
}
