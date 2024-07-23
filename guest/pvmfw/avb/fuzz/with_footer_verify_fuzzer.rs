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

use avb_bindgen::{
    AvbFooter, AvbVBMetaImageHeader, AVB_FOOTER_MAGIC, AVB_FOOTER_MAGIC_LEN, AVB_MAGIC,
    AVB_MAGIC_LEN, AVB_VERSION_MAJOR, AVB_VERSION_MINOR,
};
use libfuzzer_sys::fuzz_target;
use pvmfw_avb::verify_payload;
use std::mem::{size_of, transmute};

fuzz_target!(|kernel_and_vbmeta: &[u8]| {
    // This fuzzer is mostly supposed to catch the memory corruption in
    // VBMeta parsing. It is unlikely that the randomly generated
    // kernel can pass the kernel verification, so the value of `initrd`
    // is not so important as we won't reach initrd verification with
    // this fuzzer.
    const VBMETA_SIZE: usize = size_of::<AvbVBMetaImageHeader>();
    const RESERVED_REGION_SIZE: usize = 28;

    if kernel_and_vbmeta.len() < VBMETA_SIZE {
        return;
    }
    let kernel_size = kernel_and_vbmeta.len() - VBMETA_SIZE;
    let avb_footer = AvbFooter {
        magic: AVB_FOOTER_MAGIC[..(AVB_FOOTER_MAGIC_LEN as usize)].try_into().unwrap(),
        version_major: AVB_VERSION_MAJOR.to_be(),
        version_minor: AVB_VERSION_MINOR.to_be(),
        original_image_size: (kernel_size as u64).to_be(),
        vbmeta_offset: (kernel_size as u64).to_be(),
        vbmeta_size: (VBMETA_SIZE as u64).to_be(),
        reserved: [0u8; RESERVED_REGION_SIZE],
    };
    // SAFETY: It is safe as avb_footer is a valid AvbFooter struct.
    let avb_footer = unsafe { transmute::<AvbFooter, [u8; size_of::<AvbFooter>()]>(avb_footer) };

    let mut modified_kernel = vec![0u8; kernel_and_vbmeta.len() + size_of::<AvbFooter>()];
    modified_kernel[..kernel_and_vbmeta.len()].copy_from_slice(kernel_and_vbmeta);
    // Sets the magic for AvbVBMetaImageHeader.
    modified_kernel[kernel_size..(kernel_size + AVB_MAGIC_LEN as usize)]
        .copy_from_slice(&AVB_MAGIC[..(AVB_MAGIC_LEN as usize)]);
    modified_kernel[kernel_and_vbmeta.len()..].copy_from_slice(&avb_footer);

    let _ = verify_payload(&modified_kernel, /* initrd= */ None, &[0u8; 64]);
});
