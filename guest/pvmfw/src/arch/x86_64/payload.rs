// Copyright 2024, The Android Open Source Project
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

//! x86_64 low-level payload entry point

use crate::memory::MemorySlices;

/// Function boot payload after cleaning all secret from pvmfw memory
pub fn jump_to_payload(entrypoint: usize, slices: &MemorySlices) -> ! {
    // TODO(b/354116267): implement x86_64 payload handoff
    let _ = entrypoint;
    let _ = slices;
    todo!()
}
