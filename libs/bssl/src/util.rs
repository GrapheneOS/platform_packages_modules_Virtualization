// Copyright 2023, The Android Open Source Project
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

//! Utility functions.

use crate::err::process_error_queue;
use bssl_avf_error::{ApiName, Error, Result};
use log::error;

pub(crate) fn check_int_result(ret: i32, api_name: ApiName) -> Result<()> {
    match ret {
        1 => Ok(()),
        0 => Err(Error::CallFailed(api_name, process_error_queue())),
        _ => {
            error!(
                "Received a return value ({ret}) other than 0 or 1 from the BoringSSL API: {api_name:?}"
            );
            Err(Error::InternalError)
        }
    }
}

pub(crate) fn to_call_failed_error(api_name: ApiName) -> Error {
    Error::CallFailed(api_name, process_error_queue())
}
