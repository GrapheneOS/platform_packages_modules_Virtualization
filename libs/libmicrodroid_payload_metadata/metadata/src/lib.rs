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

//! Read/write metadata blob for VM payload image. The blob is supposed to be used as a metadata
//! partition in the VM payload image.
//! The layout of metadata blob is like:
//!   4 bytes : size(N) in big endian
//!   N bytes : protobuf message for Metadata

use anyhow::Result;
use protobuf::Message;
use std::io::Read;
use std::io::Write;

pub use microdroid_metadata::metadata::{
    metadata::Payload as PayloadMetadata, ApexPayload, ApkPayload, Metadata, PayloadConfig,
};

/// Reads a metadata from a reader
pub fn read_metadata<T: Read>(mut r: T) -> Result<Metadata> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    let size = i32::from_be_bytes(buf);
    Ok(Metadata::parse_from_reader(&mut r.take(size as u64))?)
}

/// Writes a metadata to a writer
pub fn write_metadata<T: Write>(metadata: &Metadata, mut w: T) -> Result<()> {
    let mut buf = Vec::new();
    metadata.write_to_writer(&mut buf)?;
    w.write_all(&(buf.len() as i32).to_be_bytes())?;
    w.write_all(&buf)?;
    Ok(())
}
