/*
 * Copyright (C) 2021 The Android Open Source Project
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

/// Common block and page size in Linux.
pub const CHUNK_SIZE: u64 = 4096;

pub fn divide_roundup(dividend: u64, divisor: u64) -> u64 {
    (dividend + divisor - 1) / divisor
}

/// Given `offset` and `length`, generates (offset, size) tuples that together form the same length,
/// and aligned to `alignment`.
pub struct ChunkedSizeIter {
    remaining: usize,
    offset: u64,
    alignment: usize,
}

impl ChunkedSizeIter {
    pub fn new(remaining: usize, offset: u64, alignment: usize) -> Self {
        ChunkedSizeIter { remaining, offset, alignment }
    }
}

impl Iterator for ChunkedSizeIter {
    type Item = (u64, usize);

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let chunk_data_size = std::cmp::min(
            self.remaining,
            self.alignment - (self.offset % self.alignment as u64) as usize,
        );
        let retval = (self.offset, chunk_data_size);
        self.offset += chunk_data_size as u64;
        self.remaining = self.remaining.saturating_sub(chunk_data_size);
        Some(retval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_chunk_read_iter(remaining: usize, offset: u64) -> Vec<(u64, usize)> {
        ChunkedSizeIter::new(remaining, offset, 4096).collect::<Vec<_>>()
    }

    #[test]
    fn test_chunk_read_iter() {
        assert_eq!(collect_chunk_read_iter(4096, 0), [(0, 4096)]);
        assert_eq!(collect_chunk_read_iter(8192, 0), [(0, 4096), (4096, 4096)]);
        assert_eq!(collect_chunk_read_iter(8192, 4096), [(4096, 4096), (8192, 4096)]);

        assert_eq!(
            collect_chunk_read_iter(16384, 1),
            [(1, 4095), (4096, 4096), (8192, 4096), (12288, 4096), (16384, 1)]
        );

        assert_eq!(collect_chunk_read_iter(0, 0), []);
        assert_eq!(collect_chunk_read_iter(0, 100), []);
    }
}
