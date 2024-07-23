/*
 * Copyright (C) 2022 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

// This program loads kernel and initrd which the system will boot into when
// panic occurs.

#include <errno.h>
#include <fcntl.h>
#include <linux/kexec.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

#if defined(__aarch64__)
#define EARLYCON "earlycon=uart8250,mmio,0x3f8"
#elif defined(__x86_64__)
#define EARLYCON "earlycon=uart8250,io,0x3f8"
#endif

static const char *KERNEL = "/system/etc/microdroid_crashdump_kernel";
static const char *INITRD = "/system/etc/microdroid_crashdump_initrd.img";
static const char *CMDLINE = "1 panic=-1 rdinit=/bin/crashdump nr_cpus=1 reset_devices "
                             "console=hvc0 " EARLYCON;

static int open_checked(const char *path) {
    int fd = open(path, O_RDONLY);
    if (fd == -1) {
        fprintf(stderr, "Failed to open %s: %s\n", path, strerror(errno));
        exit(1);
    }
    return fd;
}

int main() {
    unsigned long cmdline_len = strlen(CMDLINE) + 1; // include null terminator, otherwise EINVAL

    if (syscall(SYS_kexec_file_load, open_checked(KERNEL), open_checked(INITRD), cmdline_len,
                CMDLINE, KEXEC_FILE_ON_CRASH) == -1) {
        fprintf(stderr, "Failed to load panic kernel: %s\n", strerror(errno));
        if (errno == EADDRNOTAVAIL) {
            struct stat st;
            off_t kernel_size = 0;
            off_t initrd_size = 0;

            if (stat(KERNEL, &st) == 0) {
                kernel_size = st.st_size;
            }
            if (stat(INITRD, &st) == 0) {
                initrd_size = st.st_size;
            }
            fprintf(stderr, "Image size too big? %s:%ld bytes, %s:%ld bytes", KERNEL, kernel_size,
                    INITRD, initrd_size);
        }
        return 1;
    }
    return 0;
}
