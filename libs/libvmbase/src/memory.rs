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

//! Memory management.

mod dbm;
mod error;
mod page_table;
mod shared;
mod util;

pub use error::MemoryTrackerError;
pub use page_table::PageTable;
pub use shared::{
    handle_permission_fault, handle_translation_fault, MemoryRange, MemoryTracker, MEMORY,
};
pub use util::{
    flush, flushed_zeroize, min_dcache_line_size, page_4kb_of, PAGE_SIZE, SIZE_128KB, SIZE_16KB,
    SIZE_2MB, SIZE_4KB, SIZE_4MB, SIZE_64KB,
};

pub(crate) use shared::{alloc_shared, dealloc_shared};
pub(crate) use util::{phys_to_virt, virt_to_phys};
