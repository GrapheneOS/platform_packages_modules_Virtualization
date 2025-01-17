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

//! Helpers for using BoringSSL CBB (crypto byte builder) objects.

use bssl_sys::{CBB_init_fixed, CBB};
use core::marker::PhantomData;
use core::mem::MaybeUninit;

/// Wraps a CBB that references a existing fixed-sized buffer; no memory is allocated, but the
/// buffer cannot grow.
pub struct CbbFixed<'a> {
    cbb: CBB,
    /// The CBB contains a mutable reference to the buffer, disguised as a pointer.
    /// Make sure the borrow checker knows that.
    _buffer: PhantomData<&'a mut [u8]>,
}

impl<'a> CbbFixed<'a> {
    /// Create a new CBB that writes to the given buffer.
    pub fn new(buffer: &'a mut [u8]) -> Self {
        let mut cbb = MaybeUninit::uninit();
        // SAFETY: `CBB_init_fixed()` is infallible and always returns one.
        // The buffer remains valid during the lifetime of `cbb`.
        unsafe { CBB_init_fixed(cbb.as_mut_ptr(), buffer.as_mut_ptr(), buffer.len()) };
        // SAFETY: `cbb` has just been initialized by `CBB_init_fixed()`.
        let cbb = unsafe { cbb.assume_init() };
        Self { cbb, _buffer: PhantomData }
    }
}

impl AsRef<CBB> for CbbFixed<'_> {
    fn as_ref(&self) -> &CBB {
        &self.cbb
    }
}

impl AsMut<CBB> for CbbFixed<'_> {
    fn as_mut(&mut self) -> &mut CBB {
        &mut self.cbb
    }
}
