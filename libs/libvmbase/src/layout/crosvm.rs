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

//! Memory layout for crosvm for aarch64 architecture.
//!
//! https://crosvm.dev/book/appendix/memory_layout.html#common-layout

use core::ops::Range;

/// The start address of MMIO space.
pub const MMIO_START: usize = 0x0;
/// The end address of MMIO space.
pub const MMIO_END: usize = 0x4000_0000;
/// MMIO range.
pub const MMIO_RANGE: Range<usize> = MMIO_START..MMIO_END;

/// The start of the system's contiguous "main" memory.
pub const MEM_START: usize = 0x8000_0000;

/// Size of the FDT region as defined by crosvm, both in kernel and BIOS modes.
pub const FDT_MAX_SIZE: usize = 2 << 20;
