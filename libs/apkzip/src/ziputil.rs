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

//! Utilities for zip handling of APK files.

use anyhow::{ensure, Result};
use bytes::{Buf, BufMut};
use std::io::{Read, Seek};
use zip::ZipArchive;

const EOCD_SIZE_WITHOUT_COMMENT: usize = 22;
const EOCD_CENTRAL_DIRECTORY_SIZE_FIELD_OFFSET: usize = 12;
const EOCD_CENTRAL_DIRECTORY_OFFSET_FIELD_OFFSET: usize = 16;
/// End of Central Directory signature
const EOCD_SIGNATURE: u32 = 0x06054b50;
const ZIP64_MARK: u32 = 0xffffffff;

/// Information about the layout of a zip file.
#[derive(Debug, PartialEq, Eq)]
pub struct ZipSections {
    /// Offset within the file of the central directory.
    pub central_directory_offset: u32,
    /// Size of the central directory.
    pub central_directory_size: u32,
    /// Offset within the file of end of central directory marker.
    pub eocd_offset: u32,
    /// Size of the end of central directory marker.
    pub eocd_size: u32,
}

/// Discover the layout of a zip file.
pub fn zip_sections<R: Read + Seek>(mut reader: R) -> Result<ZipSections> {
    // open a zip to parse EOCD
    let archive = ZipArchive::new(reader)?;
    let eocd_size = archive.comment().len() + EOCD_SIZE_WITHOUT_COMMENT;
    ensure!(archive.offset() == 0, "Invalid ZIP: offset should be 0, but {}.", archive.offset());
    // retrieve reader back
    reader = archive.into_inner();
    // the current position should point EOCD offset
    let eocd_offset = reader.stream_position()? as u32;
    let mut eocd = vec![0u8; eocd_size];
    reader.read_exact(&mut eocd)?;
    ensure!(
        (&eocd[0..]).get_u32_le() == EOCD_SIGNATURE,
        "Invalid ZIP: ZipArchive::new() should point EOCD after reading."
    );
    let (central_directory_size, central_directory_offset) = get_central_directory(&eocd)?;
    ensure!(
        central_directory_offset != ZIP64_MARK && central_directory_size != ZIP64_MARK,
        "Unsupported ZIP: ZIP64 is not supported."
    );
    ensure!(
        central_directory_offset + central_directory_size == eocd_offset,
        "Invalid ZIP: EOCD should follow CD with no extra data or overlap."
    );

    Ok(ZipSections {
        central_directory_offset,
        central_directory_size,
        eocd_offset,
        eocd_size: eocd_size as u32,
    })
}

fn get_central_directory(buf: &[u8]) -> Result<(u32, u32)> {
    ensure!(buf.len() >= EOCD_SIZE_WITHOUT_COMMENT, "Invalid EOCD size: {}", buf.len());
    let mut buf = &buf[EOCD_CENTRAL_DIRECTORY_SIZE_FIELD_OFFSET..];
    let size = buf.get_u32_le();
    let offset = buf.get_u32_le();
    Ok((size, offset))
}

/// Update EOCD's central_directory_offset field.
pub fn set_central_directory_offset(buf: &mut [u8], value: u32) -> Result<()> {
    ensure!(buf.len() >= EOCD_SIZE_WITHOUT_COMMENT, "Invalid EOCD size: {}", buf.len());
    (&mut buf[EOCD_CENTRAL_DIRECTORY_OFFSET_FIELD_OFFSET..]).put_u32_le(value);
    Ok(())
}

/// Read an entire file from a .zip file into memory and return it.
pub fn read_file<R: Read + Seek>(reader: R, file_name: &str) -> Result<Vec<u8>> {
    let mut archive = ZipArchive::new(reader)?;
    let mut file = archive.by_name(file_name)?;
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::{write::FileOptions, ZipWriter};

    const FILE_CONTENT: &[u8] = b"testcontent";
    const FILE_NAME: &str = "testfile";

    fn create_test_zip() -> Cursor<Vec<u8>> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file(FILE_NAME, FileOptions::default()).unwrap();
        writer.write_all(FILE_CONTENT).unwrap();
        writer.finish().unwrap()
    }

    fn assert_contains(haystack: &str, needle: &str) {
        assert!(haystack.contains(needle), "{needle} is not found in {haystack}");
    }

    #[test]
    fn test_zip_sections() {
        let mut cursor = create_test_zip();
        let sections = zip_sections(&mut cursor).unwrap();
        assert_eq!(
            sections.eocd_offset,
            (cursor.get_ref().len() - EOCD_SIZE_WITHOUT_COMMENT) as u32
        );
    }

    #[test]
    fn test_read_file() {
        let file = read_file(create_test_zip(), FILE_NAME).unwrap();
        assert_eq!(file.as_slice(), FILE_CONTENT);
    }

    #[test]
    fn test_reject_if_extra_data_between_cd_and_eocd() {
        // prepare normal zip
        let buf = create_test_zip().into_inner();

        // insert garbage between CD and EOCD.
        // by the way, to mock zip-rs, use CD as garbage. This is implementation detail of zip-rs,
        // which reads CD at (eocd_offset - cd_size) instead of at cd_offset from EOCD.
        let (pre_eocd, eocd) = buf.split_at(buf.len() - EOCD_SIZE_WITHOUT_COMMENT);
        let (_, cd_offset) = get_central_directory(eocd).unwrap();
        let cd = &pre_eocd[cd_offset as usize..];

        // ZipArchive::new() succeeds, but we should reject
        let res = zip_sections(Cursor::new([pre_eocd, cd, eocd].concat()));
        assert!(res.is_err());
        assert_contains(&res.err().unwrap().to_string(), "Invalid ZIP: offset should be 0");
    }
}
