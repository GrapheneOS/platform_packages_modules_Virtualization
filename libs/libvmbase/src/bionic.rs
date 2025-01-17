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

//! Low-level compatibility layer between baremetal Rust and Bionic C functions.

use crate::rand::fill_with_entropy;
use crate::read_sysreg;
use core::ffi::c_char;
use core::ffi::c_int;
use core::ffi::c_void;
use core::ffi::CStr;
use core::slice;
use core::str;

use log::error;
use log::info;

const EOF: c_int = -1;
const EIO: c_int = 5;

/// Bionic thread-local storage.
#[repr(C)]
pub struct Tls {
    /// Unused.
    _unused: [u8; 40],
    /// Use by the compiler as stack canary value.
    pub stack_guard: u64,
}

/// Bionic TLS.
///
/// Provides the TLS used by Bionic code. This is unique as vmbase only supports one thread.
///
/// Note that the linker script re-exports __bionic_tls.stack_guard as __stack_chk_guard for
/// compatibility with non-Bionic LLVM.
#[link_section = ".data.stack_protector"]
#[export_name = "__bionic_tls"]
pub static mut TLS: Tls = Tls { _unused: [0; 40], stack_guard: 0 };

/// Gets a reference to the TLS from the dedicated system register.
pub fn __get_tls() -> &'static mut Tls {
    let tpidr = read_sysreg!("tpidr_el0");
    // SAFETY: The register is currently only written to once, from entry.S, with a valid value.
    unsafe { &mut *(tpidr as *mut Tls) }
}

#[no_mangle]
extern "C" fn __stack_chk_fail() -> ! {
    panic!("stack guard check failed");
}

/// Called from C to cause abnormal program termination.
#[no_mangle]
extern "C" fn abort() -> ! {
    panic!("C code called abort()")
}

/// Error number set and read by C functions.
pub static mut ERRNO: c_int = 0;

#[no_mangle]
// SAFETY: C functions which call this are only called from the main thread, not from exception
// handlers.
unsafe extern "C" fn __errno() -> *mut c_int {
    (&raw mut ERRNO).cast()
}

fn set_errno(value: c_int) {
    // SAFETY: vmbase is currently single-threaded.
    unsafe { ERRNO = value };
}

fn get_errno() -> c_int {
    // SAFETY: vmbase is currently single-threaded.
    unsafe { ERRNO }
}

/// # Safety
///
/// `buffer` must point to an allocation of at least `length` bytes which is valid to write to and
/// has no concurrent access while this function is running.
#[no_mangle]
unsafe extern "C" fn getentropy(buffer: *mut c_void, length: usize) -> c_int {
    if length > 256 {
        // The maximum permitted value for the length argument is 256.
        set_errno(EIO);
        return -1;
    }

    // SAFETY: The caller promised that `buffer` is a valid pointer to at least `length` bytes with
    // no concurrent access.
    let buffer = unsafe { slice::from_raw_parts_mut(buffer.cast::<u8>(), length) };
    fill_with_entropy(buffer).unwrap();

    0
}

/// Reports a fatal error detected by Bionic.
///
/// # Safety
///
/// Input strings `prefix` and `format` must be valid and properly NUL-terminated.
///
/// # Note
///
/// This Rust function is missing the last argument of its C/C++ counterpart, a va_list.
#[no_mangle]
unsafe extern "C" fn async_safe_fatal_va_list(prefix: *const c_char, format: *const c_char) {
    // SAFETY: The caller guaranteed that both strings were valid and NUL-terminated.
    let (prefix, format) = unsafe { (CStr::from_ptr(prefix), CStr::from_ptr(format)) };

    if let (Ok(prefix), Ok(format)) = (prefix.to_str(), format.to_str()) {
        // We don't bother with printf formatting.
        error!("FATAL BIONIC ERROR: {prefix}: \"{format}\" (unformatted)");
    }
}

#[cfg(target_arch = "aarch64")]
#[allow(clippy::enum_clike_unportable_variant)] // No risk if AArch64 only.
#[repr(usize)]
/// Fake FILE* values used by C to refer to the default streams.
///
/// These values are intentionally invalid pointers so that dereferencing them will be caught.
enum CFilePtr {
    // On AArch64 with TCR_EL1.EPD1 set or TCR_EL1.T1SZ > 12, these VAs can't be mapped.
    Stdout = 0xfff0_badf_badf_bad0,
    Stderr = 0xfff0_badf_badf_bad1,
}

impl CFilePtr {
    fn write_lines(&self, s: &str) {
        for line in s.split_inclusive('\n') {
            let (line, ellipsis) = if let Some(stripped) = line.strip_suffix('\n') {
                (stripped, "")
            } else {
                (line, " ...")
            };

            match self {
                Self::Stdout => info!("{line}{ellipsis}"),
                Self::Stderr => error!("{line}{ellipsis}"),
            }
        }
    }
}

impl TryFrom<usize> for CFilePtr {
    type Error = &'static str;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::Stdout as _ => Ok(Self::Stdout),
            x if x == Self::Stderr as _ => Ok(Self::Stderr),
            _ => Err("Received Invalid FILE* from C"),
        }
    }
}

#[no_mangle]
static stdout: CFilePtr = CFilePtr::Stdout;
#[no_mangle]
static stderr: CFilePtr = CFilePtr::Stderr;

/// # Safety
///
/// `c_str` must be a valid pointer to a NUL-terminated string which is not modified before this
/// function returns.
#[no_mangle]
unsafe extern "C" fn fputs(c_str: *const c_char, stream: usize) -> c_int {
    // SAFETY: The caller promised that `c_str` is a valid NUL-terminated string.
    let c_str = unsafe { CStr::from_ptr(c_str) };

    if let (Ok(s), Ok(f)) = (c_str.to_str(), CFilePtr::try_from(stream)) {
        f.write_lines(s);
        0
    } else {
        set_errno(EOF);
        EOF
    }
}

/// # Safety
///
/// `ptr` must be a valid pointer to an array of at least `size * nmemb` initialised bytes, which
/// are not modified before this function returns.
#[no_mangle]
unsafe extern "C" fn fwrite(ptr: *const c_void, size: usize, nmemb: usize, stream: usize) -> usize {
    let length = size.saturating_mul(nmemb);

    // SAFETY: The caller promised that `ptr` is a valid pointer to at least `size * nmemb`
    // initialised bytes, and `length` is no more than that.
    let bytes = unsafe { slice::from_raw_parts(ptr as *const u8, length) };

    if let (Ok(s), Ok(f)) = (str::from_utf8(bytes), CFilePtr::try_from(stream)) {
        f.write_lines(s);
        length
    } else {
        0
    }
}

#[no_mangle]
extern "C" fn strerror(n: c_int) -> *mut c_char {
    cstr_error(n).as_ptr().cast_mut().cast()
}

/// # Safety
///
/// `s` must be a valid pointer to a NUL-terminated string which is not modified before this
/// function returns.
#[no_mangle]
unsafe extern "C" fn perror(s: *const c_char) {
    let prefix = if s.is_null() {
        None
    } else {
        // SAFETY: The caller promised that `s` is a valid NUL-terminated string.
        let c_str = unsafe { CStr::from_ptr(s) };
        if c_str.is_empty() {
            None
        } else {
            Some(c_str.to_str().unwrap())
        }
    };

    let error = cstr_error(get_errno()).to_str().unwrap();

    if let Some(prefix) = prefix {
        error!("{prefix}: {error}");
    } else {
        error!("{error}");
    }
}

fn cstr_error(n: c_int) -> &'static CStr {
    // Messages taken from errno(1).
    match n {
        0 => c"Success",
        1 => c"Operation not permitted",
        2 => c"No such file or directory",
        3 => c"No such process",
        4 => c"Interrupted system call",
        5 => c"Input/output error",
        6 => c"No such device or address",
        7 => c"Argument list too long",
        8 => c"Exec format error",
        9 => c"Bad file descriptor",
        10 => c"No child processes",
        11 => c"Resource temporarily unavailable",
        12 => c"Cannot allocate memory",
        13 => c"Permission denied",
        14 => c"Bad address",
        15 => c"Block device required",
        16 => c"Device or resource busy",
        17 => c"File exists",
        18 => c"Invalid cross-device link",
        19 => c"No such device",
        20 => c"Not a directory",
        21 => c"Is a directory",
        22 => c"Invalid argument",
        23 => c"Too many open files in system",
        24 => c"Too many open files",
        25 => c"Inappropriate ioctl for device",
        26 => c"Text file busy",
        27 => c"File too large",
        28 => c"No space left on device",
        29 => c"Illegal seek",
        30 => c"Read-only file system",
        31 => c"Too many links",
        32 => c"Broken pipe",
        33 => c"Numerical argument out of domain",
        34 => c"Numerical result out of range",
        35 => c"Resource deadlock avoided",
        36 => c"File name too long",
        37 => c"No locks available",
        38 => c"Function not implemented",
        39 => c"Directory not empty",
        40 => c"Too many levels of symbolic links",
        42 => c"No message of desired type",
        43 => c"Identifier removed",
        44 => c"Channel number out of range",
        45 => c"Level 2 not synchronized",
        46 => c"Level 3 halted",
        47 => c"Level 3 reset",
        48 => c"Link number out of range",
        49 => c"Protocol driver not attached",
        50 => c"No CSI structure available",
        51 => c"Level 2 halted",
        52 => c"Invalid exchange",
        53 => c"Invalid request descriptor",
        54 => c"Exchange full",
        55 => c"No anode",
        56 => c"Invalid request code",
        57 => c"Invalid slot",
        59 => c"Bad font file format",
        60 => c"Device not a stream",
        61 => c"No data available",
        62 => c"Timer expired",
        63 => c"Out of streams resources",
        64 => c"Machine is not on the network",
        65 => c"Package not installed",
        66 => c"Object is remote",
        67 => c"Link has been severed",
        68 => c"Advertise error",
        69 => c"Srmount error",
        70 => c"Communication error on send",
        71 => c"Protocol error",
        72 => c"Multihop attempted",
        73 => c"RFS specific error",
        74 => c"Bad message",
        75 => c"Value too large for defined data type",
        76 => c"Name not unique on network",
        77 => c"File descriptor in bad state",
        78 => c"Remote address changed",
        79 => c"Can not access a needed shared library",
        80 => c"Accessing a corrupted shared library",
        81 => c".lib section in a.out corrupted",
        82 => c"Attempting to link in too many shared libraries",
        83 => c"Cannot exec a shared library directly",
        84 => c"Invalid or incomplete multibyte or wide character",
        85 => c"Interrupted system call should be restarted",
        86 => c"Streams pipe error",
        87 => c"Too many users",
        88 => c"Socket operation on non-socket",
        89 => c"Destination address required",
        90 => c"Message too long",
        91 => c"Protocol wrong type for socket",
        92 => c"Protocol not available",
        93 => c"Protocol not supported",
        94 => c"Socket type not supported",
        95 => c"Operation not supported",
        96 => c"Protocol family not supported",
        97 => c"Address family not supported by protocol",
        98 => c"Address already in use",
        99 => c"Cannot assign requested address",
        100 => c"Network is down",
        101 => c"Network is unreachable",
        102 => c"Network dropped connection on reset",
        103 => c"Software caused connection abort",
        104 => c"Connection reset by peer",
        105 => c"No buffer space available",
        106 => c"Transport endpoint is already connected",
        107 => c"Transport endpoint is not connected",
        108 => c"Cannot send after transport endpoint shutdown",
        109 => c"Too many references: cannot splice",
        110 => c"Connection timed out",
        111 => c"Connection refused",
        112 => c"Host is down",
        113 => c"No route to host",
        114 => c"Operation already in progress",
        115 => c"Operation now in progress",
        116 => c"Stale file handle",
        117 => c"Structure needs cleaning",
        118 => c"Not a XENIX named type file",
        119 => c"No XENIX semaphores available",
        120 => c"Is a named type file",
        121 => c"Remote I/O error",
        122 => c"Disk quota exceeded",
        123 => c"No medium found",
        124 => c"Wrong medium type",
        125 => c"Operation canceled",
        126 => c"Required key not available",
        127 => c"Key has expired",
        128 => c"Key has been revoked",
        129 => c"Key was rejected by service",
        130 => c"Owner died",
        131 => c"State not recoverable",
        132 => c"Operation not possible due to RF-kill",
        133 => c"Memory page has hardware error",
        _ => c"Unknown errno value",
    }
}
