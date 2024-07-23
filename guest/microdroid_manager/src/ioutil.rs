// Copyright 2021, The Android Open Source Project
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

//! IO utilities

use anyhow::{anyhow, bail, Result};
use log::debug;
use std::fmt::Debug;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

const SLEEP_DURATION: Duration = Duration::from_millis(5);

/// waits for a file with a timeout and returns it
///
/// WARNING: This only guarantees file creation. When there's another thread
///   writing a file and you're waiting for the file, reading the file should be
///   synchronized with other mechanism than just waiting for the creation.
pub fn wait_for_file<P: AsRef<Path> + Debug>(path: P, timeout: Duration) -> Result<File> {
    debug!("waiting for {:?}...", path);
    let begin = Instant::now();
    loop {
        match File::open(&path) {
            Ok(file) => return Ok(file),
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    return Err(anyhow!(error));
                }
                if begin.elapsed() > timeout {
                    return Err(anyhow!(io::Error::from(io::ErrorKind::NotFound)));
                }
                thread::sleep(SLEEP_DURATION);
            }
        }
    }
}

// From include/uapi/linux/fs.h
const BLK: u8 = 0x12;
const BLKFLSBUF: u8 = 97;
nix::ioctl_none!(_blkflsbuf, BLK, BLKFLSBUF);

pub fn blkflsbuf(f: &mut File) -> Result<()> {
    if !f.metadata()?.file_type().is_block_device() {
        bail!("{:?} is not a block device", f.as_raw_fd());
    }
    // SAFETY: The file is kept open until the end of this function.
    unsafe { _blkflsbuf(f.as_raw_fd()) }?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::rename;
    use std::io::{Read, Write};

    #[test]
    fn test_wait_for_file() -> Result<()> {
        let test_dir = tempfile::TempDir::new().unwrap();
        let test_file = test_dir.path().join("test.txt");
        let temp_file = test_dir.path().join("test.txt~");
        thread::spawn(move || -> io::Result<()> {
            // Sleep to ensure that `wait_for_file` actually waits
            thread::sleep(Duration::from_secs(1));
            // Write to a temp file and then rename it to avoid the race between
            // write and read.
            File::create(&temp_file)?.write_all(b"test")?;
            rename(temp_file, test_file)
        });

        let test_file = test_dir.path().join("test.txt");
        let mut file = wait_for_file(test_file, Duration::from_secs(5))?;
        let mut buffer = String::new();
        file.read_to_string(&mut buffer)?;
        assert_eq!("test", buffer);
        Ok(())
    }

    #[test]
    fn test_wait_for_file_fails() {
        let test_dir = tempfile::TempDir::new().unwrap();
        let test_file = test_dir.path().join("test.txt");
        let file = wait_for_file(test_file, Duration::from_secs(1));
        assert!(file.is_err());
        assert_eq!(
            io::ErrorKind::NotFound,
            file.unwrap_err().root_cause().downcast_ref::<io::Error>().unwrap().kind()
        );
    }
}
