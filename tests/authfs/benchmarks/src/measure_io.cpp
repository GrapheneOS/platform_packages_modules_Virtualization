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

#include <android-base/unique_fd.h>
#include <assert.h>
#include <err.h>
#include <fcntl.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <algorithm>
#include <iomanip>
#include <iostream>
#include <random>

using android::base::unique_fd;

constexpr int kBlockSizeBytes = 4096;
constexpr int kNumBytesPerMB = 1024 * 1024;

int main(int argc, const char *argv[]) {
    if (argc != 5 || !(strcmp(argv[3], "rand") == 0 || strcmp(argv[3], "seq") == 0) ||
        !(strcmp(argv[4], "r") == 0 || strcmp(argv[4], "w") == 0)) {
        errx(EXIT_FAILURE, "Usage: %s <filename> <file_size_mb> <rand|seq> <r|w>", argv[0]);
    }
    int file_size_mb = std::stoi(argv[2]);
    bool is_rand = (strcmp(argv[3], "rand") == 0);
    bool is_read = (strcmp(argv[4], "r") == 0);
    const int block_count = file_size_mb * kNumBytesPerMB / kBlockSizeBytes;
    std::vector<int> offsets(block_count);
    for (auto i = 0; i < block_count; ++i) {
        offsets[i] = i * kBlockSizeBytes;
    }
    if (is_rand) {
        std::mt19937 rd{std::random_device{}()};
        std::shuffle(offsets.begin(), offsets.end(), rd);
    }
    unique_fd fd(open(argv[1], (is_read ? O_RDONLY : O_WRONLY) | O_CLOEXEC));
    if (fd.get() == -1) {
        errx(EXIT_FAILURE, "failed to open file: %s", argv[1]);
    }

    char buf[kBlockSizeBytes];
    struct timespec start;
    if (clock_gettime(CLOCK_MONOTONIC, &start) == -1) {
        err(EXIT_FAILURE, "failed to clock_gettime");
    }
    for (auto i = 0; i < block_count; ++i) {
        auto bytes = is_read ? pread(fd, buf, kBlockSizeBytes, offsets[i])
                             : pwrite(fd, buf, kBlockSizeBytes, offsets[i]);
        if (bytes == 0) {
            errx(EXIT_FAILURE, "unexpected end of file");
        } else if (bytes == -1) {
            errx(EXIT_FAILURE, "failed to read");
        }
    }
    if (!is_read) {
        // Writes all the buffered modifications to the open file.
        assert(syncfs(fd) == 0);
    }
    struct timespec finish;
    if (clock_gettime(CLOCK_MONOTONIC, &finish) == -1) {
        err(EXIT_FAILURE, "failed to clock_gettime");
    }
    double elapsed_seconds = finish.tv_sec - start.tv_sec + (finish.tv_nsec - start.tv_nsec) / 1e9;
    double rate = (double)file_size_mb / elapsed_seconds;
    std::cout << std::setprecision(12) << rate << std::endl;

    return EXIT_SUCCESS;
}
