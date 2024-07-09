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
use core::ptr::addr_of_mut;
use core::slice;
use core::str;

use cstr::cstr;
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
unsafe extern "C" fn __errno() -> *mut c_int {
    // SAFETY: C functions which call this are only called from the main thread, not from exception
    // handlers.
    unsafe { addr_of_mut!(ERRNO) as *mut _ }
}

fn set_errno(value: c_int) {
    // SAFETY: vmbase is currently single-threaded.
    unsafe { ERRNO = value };
}

fn get_errno() -> c_int {
    // SAFETY: vmbase is currently single-threaded.
    unsafe { ERRNO }
}

#[no_mangle]
extern "C" fn getentropy(buffer: *mut c_void, length: usize) -> c_int {
    if length > 256 {
        // The maximum permitted value for the length argument is 256.
        set_errno(EIO);
        return -1;
    }

    // SAFETY: Just like libc, we need to assume that `ptr` is valid.
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

#[no_mangle]
extern "C" fn fputs(c_str: *const c_char, stream: usize) -> c_int {
    // SAFETY: Just like libc, we need to assume that `s` is a valid NULL-terminated string.
    let c_str = unsafe { CStr::from_ptr(c_str) };

    if let (Ok(s), Ok(f)) = (c_str.to_str(), CFilePtr::try_from(stream)) {
        f.write_lines(s);
        0
    } else {
        set_errno(EOF);
        EOF
    }
}

#[no_mangle]
extern "C" fn fwrite(ptr: *const c_void, size: usize, nmemb: usize, stream: usize) -> usize {
    let length = size.saturating_mul(nmemb);

    // SAFETY: Just like libc, we need to assume that `ptr` is valid.
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

#[no_mangle]
extern "C" fn perror(s: *const c_char) {
    let prefix = if s.is_null() {
        None
    } else {
        // SAFETY: Just like libc, we need to assume that `s` is a valid NULL-terminated string.
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
        0 => cstr!("Success"),
        1 => cstr!("Operation not permitted"),
        2 => cstr!("No such file or directory"),
        3 => cstr!("No such process"),
        4 => cstr!("Interrupted system call"),
        5 => cstr!("Input/output error"),
        6 => cstr!("No such device or address"),
        7 => cstr!("Argument list too long"),
        8 => cstr!("Exec format error"),
        9 => cstr!("Bad file descriptor"),
        10 => cstr!("No child processes"),
        11 => cstr!("Resource temporarily unavailable"),
        12 => cstr!("Cannot allocate memory"),
        13 => cstr!("Permission denied"),
        14 => cstr!("Bad address"),
        15 => cstr!("Block device required"),
        16 => cstr!("Device or resource busy"),
        17 => cstr!("File exists"),
        18 => cstr!("Invalid cross-device link"),
        19 => cstr!("No such device"),
        20 => cstr!("Not a directory"),
        21 => cstr!("Is a directory"),
        22 => cstr!("Invalid argument"),
        23 => cstr!("Too many open files in system"),
        24 => cstr!("Too many open files"),
        25 => cstr!("Inappropriate ioctl for device"),
        26 => cstr!("Text file busy"),
        27 => cstr!("File too large"),
        28 => cstr!("No space left on device"),
        29 => cstr!("Illegal seek"),
        30 => cstr!("Read-only file system"),
        31 => cstr!("Too many links"),
        32 => cstr!("Broken pipe"),
        33 => cstr!("Numerical argument out of domain"),
        34 => cstr!("Numerical result out of range"),
        35 => cstr!("Resource deadlock avoided"),
        36 => cstr!("File name too long"),
        37 => cstr!("No locks available"),
        38 => cstr!("Function not implemented"),
        39 => cstr!("Directory not empty"),
        40 => cstr!("Too many levels of symbolic links"),
        42 => cstr!("No message of desired type"),
        43 => cstr!("Identifier removed"),
        44 => cstr!("Channel number out of range"),
        45 => cstr!("Level 2 not synchronized"),
        46 => cstr!("Level 3 halted"),
        47 => cstr!("Level 3 reset"),
        48 => cstr!("Link number out of range"),
        49 => cstr!("Protocol driver not attached"),
        50 => cstr!("No CSI structure available"),
        51 => cstr!("Level 2 halted"),
        52 => cstr!("Invalid exchange"),
        53 => cstr!("Invalid request descriptor"),
        54 => cstr!("Exchange full"),
        55 => cstr!("No anode"),
        56 => cstr!("Invalid request code"),
        57 => cstr!("Invalid slot"),
        59 => cstr!("Bad font file format"),
        60 => cstr!("Device not a stream"),
        61 => cstr!("No data available"),
        62 => cstr!("Timer expired"),
        63 => cstr!("Out of streams resources"),
        64 => cstr!("Machine is not on the network"),
        65 => cstr!("Package not installed"),
        66 => cstr!("Object is remote"),
        67 => cstr!("Link has been severed"),
        68 => cstr!("Advertise error"),
        69 => cstr!("Srmount error"),
        70 => cstr!("Communication error on send"),
        71 => cstr!("Protocol error"),
        72 => cstr!("Multihop attempted"),
        73 => cstr!("RFS specific error"),
        74 => cstr!("Bad message"),
        75 => cstr!("Value too large for defined data type"),
        76 => cstr!("Name not unique on network"),
        77 => cstr!("File descriptor in bad state"),
        78 => cstr!("Remote address changed"),
        79 => cstr!("Can not access a needed shared library"),
        80 => cstr!("Accessing a corrupted shared library"),
        81 => cstr!(".lib section in a.out corrupted"),
        82 => cstr!("Attempting to link in too many shared libraries"),
        83 => cstr!("Cannot exec a shared library directly"),
        84 => cstr!("Invalid or incomplete multibyte or wide character"),
        85 => cstr!("Interrupted system call should be restarted"),
        86 => cstr!("Streams pipe error"),
        87 => cstr!("Too many users"),
        88 => cstr!("Socket operation on non-socket"),
        89 => cstr!("Destination address required"),
        90 => cstr!("Message too long"),
        91 => cstr!("Protocol wrong type for socket"),
        92 => cstr!("Protocol not available"),
        93 => cstr!("Protocol not supported"),
        94 => cstr!("Socket type not supported"),
        95 => cstr!("Operation not supported"),
        96 => cstr!("Protocol family not supported"),
        97 => cstr!("Address family not supported by protocol"),
        98 => cstr!("Address already in use"),
        99 => cstr!("Cannot assign requested address"),
        100 => cstr!("Network is down"),
        101 => cstr!("Network is unreachable"),
        102 => cstr!("Network dropped connection on reset"),
        103 => cstr!("Software caused connection abort"),
        104 => cstr!("Connection reset by peer"),
        105 => cstr!("No buffer space available"),
        106 => cstr!("Transport endpoint is already connected"),
        107 => cstr!("Transport endpoint is not connected"),
        108 => cstr!("Cannot send after transport endpoint shutdown"),
        109 => cstr!("Too many references: cannot splice"),
        110 => cstr!("Connection timed out"),
        111 => cstr!("Connection refused"),
        112 => cstr!("Host is down"),
        113 => cstr!("No route to host"),
        114 => cstr!("Operation already in progress"),
        115 => cstr!("Operation now in progress"),
        116 => cstr!("Stale file handle"),
        117 => cstr!("Structure needs cleaning"),
        118 => cstr!("Not a XENIX named type file"),
        119 => cstr!("No XENIX semaphores available"),
        120 => cstr!("Is a named type file"),
        121 => cstr!("Remote I/O error"),
        122 => cstr!("Disk quota exceeded"),
        123 => cstr!("No medium found"),
        124 => cstr!("Wrong medium type"),
        125 => cstr!("Operation canceled"),
        126 => cstr!("Required key not available"),
        127 => cstr!("Key has expired"),
        128 => cstr!("Key has been revoked"),
        129 => cstr!("Key was rejected by service"),
        130 => cstr!("Owner died"),
        131 => cstr!("State not recoverable"),
        132 => cstr!("Operation not possible due to RF-kill"),
        133 => cstr!("Memory page has hardware error"),
        _ => cstr!("Unknown errno value"),
    }
}
