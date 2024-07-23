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

#![allow(missing_docs)]
#![no_main]

use libfuzzer_sys::fuzz_target;
use pvmfw_avb::verify_payload;

fuzz_target!(|kernel: &[u8]| {
    // This fuzzer is mostly supposed to catch the memory corruption in
    // AVB footer parsing. It is unlikely that the randomly generated
    // kernel can pass the kernel verification, so the value of `initrd`
    // is not so important as we won't reach initrd verification with
    // this fuzzer.
    let _ = verify_payload(kernel, /* initrd= */ None, &[0u8; 64]);
});
