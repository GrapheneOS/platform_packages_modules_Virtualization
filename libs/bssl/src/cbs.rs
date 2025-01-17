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

//! Helpers for using BoringSSL CBS (crypto byte string) objects.

use bssl_sys::{CBS_init, CBS};
use core::marker::PhantomData;
use core::mem::MaybeUninit;

/// CRYPTO ByteString.
///
/// Wraps a `CBS` that references an existing fixed-sized buffer; no memory is allocated, but the
/// buffer cannot grow.
pub struct Cbs<'a> {
    cbs: CBS,
    /// The CBS contains a mutable reference to the buffer, disguised as a pointer.
    /// Make sure the borrow checker knows that.
    _buffer: PhantomData<&'a [u8]>,
}

impl<'a> Cbs<'a> {
    /// Creates a new CBS that points to the given buffer.
    pub fn new(buffer: &'a [u8]) -> Self {
        let mut cbs = MaybeUninit::uninit();
        // SAFETY: `CBS_init()` only sets `cbs` to point to `buffer`. It doesn't take ownership
        // of data.
        unsafe { CBS_init(cbs.as_mut_ptr(), buffer.as_ptr(), buffer.len()) };
        // SAFETY: `cbs` has just been initialized by `CBS_init()`.
        let cbs = unsafe { cbs.assume_init() };
        Self { cbs, _buffer: PhantomData }
    }
}

impl AsRef<CBS> for Cbs<'_> {
    fn as_ref(&self) -> &CBS {
        &self.cbs
    }
}

impl AsMut<CBS> for Cbs<'_> {
    fn as_mut(&mut self) -> &mut CBS {
        &mut self.cbs
    }
}
