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

//! Linker-defined symbols.

extern "C" {
    /// Stack canary value
    pub static __stack_chk_guard: u64;
    /// First byte beyond the pre-loaded binary.
    pub static bin_end: u8;
    /// First byte of the `.bss` section.
    pub static bss_begin: u8;
    /// First byte beyond the `.bss` section.
    pub static bss_end: u8;
    /// First byte of the (loaded) `.data` section.
    pub static data_begin: u8;
    /// First byte beyond the (loaded) `.data` section.
    pub static data_end: u8;
    /// First byte of the pre-loaded `.data` section.
    pub static data_lma: u8;
    /// First byte of the `.dtb` section.
    pub static dtb_begin: u8;
    /// First byte beyond the `.dtb` section.
    pub static dtb_end: u8;
    /// First byte of the region available for the exception handler stack.
    pub static eh_stack_limit: u8;
    /// First byte past the region available for the stack.
    pub static init_stack_pointer: u8;
    /// First byte of the `.rodata` section.
    pub static rodata_begin: u8;
    /// First byte beyond the `.rodata` section.
    pub static rodata_end: u8;
    /// First byte of the region available for the stack.
    pub static stack_limit: u8;
    /// First byte of the `.text` section.
    pub static text_begin: u8;
    /// First byte beyond the `.text` section.
    pub static text_end: u8;
}
