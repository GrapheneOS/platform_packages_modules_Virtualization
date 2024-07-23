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

// This program runs as init in the crash kernel.

#include <errno.h>
#include <fcntl.h>
#include <linux/reboot.h>
#include <stdio.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <termios.h>
#include <unistd.h>

#define DUMP_SOURCE "/proc/vmcore"
#define DUMP_TARGET "/dev/hvc1" // See virtualizationserice/crosvm.rs
#define BUF_SIZE 4096

#define FAIL(format, ...)                                                \
    {                                                                    \
        fprintf(stderr, format ":%s\n", ##__VA_ARGS__, strerror(errno)); \
        goto fail;                                                       \
    }

// Why declare? __reboot() is the Bionic's system call stub for the reboot syscall. It is
// automatically generated (and is part of API), but Bionic doesn't export this in its headers.
extern int __reboot(int, int, int, void*);

int main() {
    // Disable buffering for better display of the progress
    if (setvbuf(stdout, NULL, _IONBF, 0) != 0) {
        fprintf(stderr, "Failed to disable buffering for stdout: %s\n", strerror(errno));
        // This isn't a critical error. Continue.
    }

    printf("Crashdump started\n");

    if (mount("proc", "/proc", "proc", 0, NULL) == -1) {
        FAIL("Failed to mount /proc");
    }

    if (mount("devtmpfs", "/dev", "devtmpfs", 0, NULL) == -1) {
        FAIL("Failed to mount /dev");
    }

    int vmcore = open(DUMP_SOURCE, O_RDONLY);
    if (vmcore == -1) {
        FAIL("Failed to open %s", DUMP_SOURCE);
    }

    int dest = open(DUMP_TARGET, O_WRONLY);
    if (dest == -1) {
        FAIL("Failed to open %s", DUMP_TARGET);
    }

    // We need to turn the line discipline off, otherwise the virtio-console will automatically
    // append more data than what we have written because some will be recognized as a control
    // sequence.
    struct termios term;
    if (tcgetattr(dest, &term) != 0) {
        FAIL("Failed to get termios for %s", DUMP_TARGET);
    }

    cfmakeraw(&term); // Always successful. Returns void.

    if (tcsetattr(dest, TCSAFLUSH, &term) != 0) {
        FAIL("Failed to set terminal to the raw mode for %s", DUMP_TARGET);
    }

    struct stat statbuf;
    if (fstat(vmcore, &statbuf) == -1) {
        FAIL("Failed to stat %s", DUMP_SOURCE);
    }
    printf("Size is %ld bytes\n", statbuf.st_size);

    // sendfile(2) is faster, can't be used because /proc/vmcore doesn't support splice_read
    size_t dumped = 0;
    char buf[BUF_SIZE];
    int progress = 0; // percentage

    while (dumped < statbuf.st_size) {
        ssize_t read_bytes = read(vmcore, buf, BUF_SIZE);
        if (read_bytes == -1) {
            FAIL("Failed to read from %s", DUMP_SOURCE);
        }
        ssize_t written_bytes = write(dest, buf, read_bytes);
        if (written_bytes == -1) {
            FAIL("Failed to write to %s", DUMP_TARGET);
        }
        dumped += written_bytes;
        int new_progress = dumped * 100 / statbuf.st_size;
        if (new_progress > progress) {
            progress = new_progress;
            printf(".");
        }
    }
    printf("done\n");

    __reboot(LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2, LINUX_REBOOT_CMD_RESTART2, "kernel panic");
    // Never reach here

fail:
    printf("Crashdump failed\n");
    return 1;
}
