# vmbase

This directory contains a Rust crate and static library which can be used to write `no_std` Rust
binaries to run in an aarch64 VM under crosvm (via the VirtualizationService), such as for pVM
firmware, a VM bootloader or kernel.

In particular it provides:

- An [entry point](entry.S) that initializes the MMU with a hard-coded identity mapping, enables the
  cache, prepares the image and allocates a stack.
- An [exception vector](exceptions.S) to call your exception handlers.
- A UART driver and `println!` macro for early console logging.
- Functions to shutdown or reboot the VM.

Libraries are also available for heap allocation, page table manipulation and PSCI calls.

## Usage

The [example](example/) subdirectory contains an example of how to use it for a VM bootloader.

### Build file

Start by creating a `rust_ffi_static` rule containing your main module:

```soong
rust_ffi_static {
    name: "libvmbase_example",
    defaults: ["vmbase_ffi_defaults"],
    crate_name: "vmbase_example",
    srcs: ["src/main.rs"],
    rustlibs: [
        "libvmbase",
    ],
}
```

`vmbase_ffi_defaults`, among other things, specifies the stdlibs including the `compiler_builtins`
and `core` crate. These must be explicitly specified as we don't want the normal set of libraries
used for a C++ binary intended to run in Android userspace.

### Entry point

Your main module needs to specify a couple of special attributes:

```rust
#![no_main]
#![no_std]
```

This tells rustc that it doesn't depend on `std`, and won't have the usual `main` function as an
entry point. Instead, `vmbase` provides a macro to specify your main function:

```rust
use vmbase::{logger, main};
use log::{info, LevelFilter};

main!(main);

pub fn main(arg0: u64, arg1: u64, arg2: u64, arg3: u64) {
    logger::init(LevelFilter::Info).unwrap();
    info!("Hello world");
}
```

vmbase adds a wrapper around your main function to initialize the console driver first (with the
UART at base address `0x3f8`, the first UART allocated by crosvm), and make a PSCI `SYSTEM_OFF` call
to shutdown the VM if your main function ever returns.

You can also shutdown the VM by calling `vmbase::power::shutdown` or 'reboot' by calling
`vmbase::power::reboot`. Either will cause crosvm to terminate the VM, but by convention we use
shutdown to indicate that the VM has finished cleanly, and reboot to indicate an error condition.

### Exception handlers

You must provide handlers for each of the 8 types of exceptions which can occur on aarch64. These
must use the C ABI, and have the expected names. For example, to log sync exceptions and reboot:

```rust
use vmbase::power::reboot;

extern "C" fn sync_exception_current() {
    eprintln!("sync_exception_current");

    let mut esr: u64;
    unsafe {
        asm!("mrs {esr}, esr_el1", esr = out(reg) esr);
    }
    eprintln!("esr={:#08x}", esr);

    reboot();
}
```

The `println!` macro shouldn't be used in exception handlers, because it relies on a global instance
of the UART driver which might be locked when the exception happens, which would result in deadlock.
Instead you can use `eprintln!`, which will re-initialize the UART every time to ensure that it can
be used. This should still be used with care, as it may interfere with whatever the rest of the
program is doing with the UART.

See [example/src/exceptions.rs](examples/src/exceptions.rs) for a complete example.

### Linker script and initial idmap

The [entry point](entry.S) code expects to be provided a hardcoded identity-mapped page table to use
initially. This must contain at least the region where the image itself is loaded, some writable
DRAM to use for the `.bss` and `.data` sections and stack, and a device mapping for the UART MMIO
region. See the [example/idmap.S](example/idmap.S) for an example of how this can be constructed.

The addresses in the pagetable must map the addresses the image is linked at, as we don't support
relocation. This can be achieved with a linker script, like the one in
[example/image.ld](example/image.ld). The key part is the regions provided to be used for the image
and writable data:

```ld
MEMORY
{
	image		: ORIGIN = 0x80200000, LENGTH = 2M
	writable_data	: ORIGIN = 0x80400000, LENGTH = 2M
}
```

### Building a binary

To link your Rust code together with the entry point code and idmap into a static binary, you need
to use a `cc_binary` rule:

```soong
cc_binary {
    name: "vmbase_example",
    defaults: ["vmbase_elf_defaults"],
    srcs: [
        "idmap.S",
    ],
    static_libs: [
        "libvmbase_example",
    ],
    linker_scripts: [
        "image.ld",
        ":vmbase_sections",
    ],
}
```

This takes your Rust library (`libvmbase_example`), the vmbase library entry point and exception
vector (`libvmbase_entry`) and your initial idmap (`idmap.S`) and builds a static binary with your
linker script (`image.ld`) and the one provided by vmbase ([`sections.ld`](sections.ld)). This is an
ELF binary, but to run it as a VM bootloader you need to `objcopy` it to a raw binary image instead,
which you can do with a `raw_binary` rule:

```soong
raw_binary {
    name: "vmbase_example_bin",
    stem: "vmbase_example.bin",
    src: ":vmbase_example",
    enabled: false,
    target: {
        android_arm64: {
            enabled: true,
        },
    },
}
```

The resulting binary can then be used to start a VM by passing it as the bootloader in a
`VirtualMachineRawConfig`.
