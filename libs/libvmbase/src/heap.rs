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

//! Heap implementation.

use alloc::alloc::alloc;
use alloc::alloc::Layout;
use alloc::boxed::Box;

use core::alloc::GlobalAlloc as _;
use core::ffi::c_void;
use core::mem;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::ptr;
use core::ptr::NonNull;

use buddy_system_allocator::LockedHeap;
use spin::{
    mutex::{SpinMutex, SpinMutexGuard},
    Once,
};

/// Configures the size of the global allocator.
#[macro_export]
macro_rules! configure_heap {
    ($len:expr) => {
        static __HEAP: $crate::heap::HeapArray<{ $len }> = $crate::heap::HeapArray::new();
        #[export_name = "get_heap"]
        fn __get_heap() -> &'static mut [u8] {
            __HEAP.get()
        }
    };
}

/// An array to be used as a heap.
///
/// This should be stored in a static variable to have the appropriate lifetime.
pub struct HeapArray<const SIZE: usize> {
    array: SpinMutex<[u8; SIZE]>,
}

impl<const SIZE: usize> HeapArray<SIZE> {
    /// Creates a new empty heap array.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self { array: SpinMutex::new([0; SIZE]) }
    }

    /// Gets the heap as a slice.
    ///
    /// Panics if called more than once.
    pub fn get(&self) -> &mut [u8] {
        SpinMutexGuard::leak(self.array.try_lock().expect("Page heap was already taken"))
            .as_mut_slice()
    }
}

extern "Rust" {
    /// Gets slice used by the global allocator, configured using configure_heap!().
    ///
    /// Panics if called more than once.
    fn get_heap() -> &'static mut [u8];
}

#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap<32> = LockedHeap::<32>::new();

/// The range of addresses used for the heap.
static HEAP_RANGE: Once<Range<usize>> = Once::new();

/// Initialize the global allocator.
///
/// Panics if called more than once.
pub(crate) fn init() {
    // SAFETY: This is in fact a safe Rust function.
    let heap = unsafe { get_heap() };

    HEAP_RANGE.call_once(|| {
        let range = heap.as_ptr_range();
        range.start as usize..range.end as usize
    });

    let start = heap.as_mut_ptr() as usize;
    let size = heap.len();

    let mut heap = HEAP_ALLOCATOR.lock();
    // SAFETY: We are supplying a valid memory range, and we only do this once.
    unsafe { heap.init(start, size) };
}

/// Allocate an aligned but uninitialized slice of heap.
pub fn aligned_boxed_slice(size: usize, align: usize) -> Option<Box<[u8]>> {
    let size = NonZeroUsize::new(size)?.get();
    let layout = Layout::from_size_align(size, align).ok()?;
    // SAFETY: We verify that `size` and the returned `ptr` are non-null.
    let ptr = unsafe { alloc(layout) };
    let ptr = NonNull::new(ptr)?.as_ptr();
    let slice_ptr = ptr::slice_from_raw_parts_mut(ptr, size);

    // SAFETY: The memory was allocated using the proper layout by our global_allocator.
    Some(unsafe { Box::from_raw(slice_ptr) })
}

#[no_mangle]
unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
    allocate(size, false).map_or(ptr::null_mut(), |p| p.cast::<c_void>().as_ptr())
}

#[no_mangle]
unsafe extern "C" fn calloc(nmemb: usize, size: usize) -> *mut c_void {
    let Some(size) = nmemb.checked_mul(size) else { return ptr::null_mut() };
    allocate(size, true).map_or(ptr::null_mut(), |p| p.cast::<c_void>().as_ptr())
}

#[no_mangle]
unsafe extern "C" fn __memset_chk(
    dest: *mut c_void,
    val: u8,
    len: usize,
    destlen: usize,
) -> *mut c_void {
    assert!(len <= destlen, "memset buffer overflow detected");
    // SAFETY: `dest` is valid for writes of `len` bytes.
    unsafe {
        ptr::write_bytes(dest, val, len);
    }
    dest
}

#[no_mangle]
/// SAFETY: ptr must be null or point to a currently-allocated block returned by allocate (either
/// directly or via malloc or calloc). Note that this function is called directly from C, so we have
/// to trust that the C code is doing the right thing; there are checks below which will catch some
/// errors.
unsafe extern "C" fn free(ptr: *mut c_void) {
    let Some(ptr) = NonNull::new(ptr) else { return };
    let heap_range = HEAP_RANGE.get().expect("free called before heap was initialised");
    assert!(
        heap_range.contains(&(ptr.as_ptr() as usize)),
        "free() called on a pointer that is not part of the HEAP: {ptr:?}"
    );
    // SAFETY: ptr is non-null and was allocated by allocate, which prepends a correctly aligned
    // usize.
    let (ptr, size) = unsafe {
        let ptr = ptr.cast::<usize>().as_ptr().offset(-1);
        (ptr, *ptr)
    };
    let size = NonZeroUsize::new(size).unwrap();
    let layout = malloc_layout(size).unwrap();
    // SAFETY: If our precondition is satisfied, then this is a valid currently-allocated block.
    unsafe { HEAP_ALLOCATOR.dealloc(ptr as *mut u8, layout) }
}

/// Allocate a block of memory suitable to return from `malloc()` etc. Returns a valid pointer
/// to a suitable aligned region of size bytes, optionally zeroed (and otherwise uninitialized), or
/// None if size is 0 or allocation fails. The block can be freed by passing the returned pointer to
/// `free()`.
fn allocate(size: usize, zeroed: bool) -> Option<NonNull<usize>> {
    let size = NonZeroUsize::new(size)?.checked_add(mem::size_of::<usize>())?;
    let layout = malloc_layout(size)?;
    // SAFETY: layout is known to have non-zero size.
    let ptr = unsafe {
        if zeroed {
            HEAP_ALLOCATOR.alloc_zeroed(layout)
        } else {
            HEAP_ALLOCATOR.alloc(layout)
        }
    };
    let ptr = NonNull::new(ptr)?.cast::<usize>().as_ptr();
    // SAFETY: ptr points to a newly allocated block of memory which is properly aligned
    // for a usize and is big enough to hold a usize as well as the requested number of
    // bytes.
    unsafe {
        *ptr = size.get();
        NonNull::new(ptr.offset(1))
    }
}

fn malloc_layout(size: NonZeroUsize) -> Option<Layout> {
    // We want at least 8 byte alignment, and we need to be able to store a usize.
    const ALIGN: usize = const_max_size(mem::size_of::<usize>(), mem::size_of::<u64>());
    Layout::from_size_align(size.get(), ALIGN).ok()
}

const fn const_max_size(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}
