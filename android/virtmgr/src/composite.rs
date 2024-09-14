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

//! Functions for creating a composite disk image.

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::Partition::Partition;
use anyhow::{bail, Context, Error};
use disk::{create_composite_disk, ImagePartitionType, PartitionInfo};
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::FileExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use zerocopy::AsBytes;
use zerocopy::FromBytes;
use zerocopy::FromZeroes;

use uuid::Uuid;

/// Constructs a composite disk image for the given list of partitions, and opens it ready to use.
///
/// Returns the composite disk image file, and a list of files whose file descriptors must be passed
/// to any process which wants to use it. This is necessary because the composite image contains
/// paths of the form `/proc/self/fd/N` for the partition images.
pub fn make_composite_image(
    partitions: &[Partition],
    zero_filler_path: &Path,
    output_path: &Path,
    header_path: &Path,
    footer_path: &Path,
) -> Result<(File, Vec<File>), Error> {
    let (partitions, mut files) = convert_partitions(partitions)?;

    let mut composite_image = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(output_path)
        .with_context(|| format!("Failed to create composite image {:?}", output_path))?;
    let mut header_file =
        OpenOptions::new().create_new(true).read(true).write(true).open(header_path).with_context(
            || format!("Failed to create composite image header {:?}", header_path),
        )?;
    let mut footer_file =
        OpenOptions::new().create_new(true).read(true).write(true).open(footer_path).with_context(
            || format!("Failed to create composite image header {:?}", footer_path),
        )?;
    let zero_filler_file = File::open(zero_filler_path).with_context(|| {
        format!("Failed to open composite image zero filler {:?}", zero_filler_path)
    })?;

    create_composite_disk(
        &partitions,
        &fd_path_for_file(&zero_filler_file),
        &fd_path_for_file(&header_file),
        &mut header_file,
        &fd_path_for_file(&footer_file),
        &mut footer_file,
        &mut composite_image,
    )?;

    // Re-open the composite image as read-only.
    let composite_image = File::open(output_path)
        .with_context(|| format!("Failed to open composite image {:?}", output_path))?;

    files.push(header_file);
    files.push(footer_file);
    files.push(zero_filler_file);

    Ok((composite_image, files))
}

/// Given the AIDL config containing a list of partitions, with a [`ParcelFileDescriptor`] for each
/// partition, returns the corresponding list of PartitionInfo and the list of files whose file
/// descriptors must be passed to any process using the composite image.
fn convert_partitions(partitions: &[Partition]) -> Result<(Vec<PartitionInfo>, Vec<File>), Error> {
    // File descriptors to pass to child process.
    let mut files = vec![];

    let partitions = partitions
        .iter()
        .map(|partition| {
            // TODO(b/187187765): This shouldn't be an Option.
            let file = partition
                .image
                .as_ref()
                .context("Invalid partition image file descriptor")?
                .as_ref()
                .try_clone()
                .context("Failed to clone partition image file descriptor")?
                .into();
            let path = fd_path_for_file(&file);
            let size = get_partition_size(&file)?;
            files.push(file);

            Ok(PartitionInfo {
                label: partition.label.to_owned(),
                path,
                partition_type: ImagePartitionType::LinuxFilesystem,
                writable: partition.writable,
                size,
                part_guid: partition.guid.as_deref().map(Uuid::parse_str).transpose()?,
            })
        })
        .collect::<Result<_, Error>>()?;

    Ok((partitions, files))
}

fn fd_path_for_file(file: &File) -> PathBuf {
    let fd = file.as_raw_fd();
    format!("/proc/self/fd/{}", fd).into()
}

/// Find the size of the partition image in the given file by parsing the header.
///
/// This will work for raw and Android sparse images. QCOW2 and composite images aren't supported.
fn get_partition_size(file: &File) -> Result<u64, Error> {
    match detect_image_type(file).context("failed to detect partition image type")? {
        ImageType::Raw => Ok(file.metadata().context("failed to get metadata")?.len()),
        ImageType::AndroidSparse => {
            // Source: system/core/libsparse/sparse_format.h
            #[repr(C)]
            #[derive(Clone, Copy, Debug, AsBytes, FromZeroes, FromBytes)]
            struct SparseHeader {
                magic: u32,
                major_version: u16,
                minor_version: u16,
                file_hdr_sz: u16,
                chunk_hdr_size: u16,
                blk_sz: u32,
                total_blks: u32,
                total_chunks: u32,
                image_checksum: u32,
            }
            let mut header = SparseHeader::new_zeroed();
            file.read_exact_at(header.as_bytes_mut(), 0)
                .context("failed to read android sparse header")?;
            let len = u64::from(header.total_blks)
                .checked_mul(header.blk_sz.into())
                .context("android sparse image len too big")?;
            Ok(len)
        }
        t => bail!("unsupported partition image type: {t:?}"),
    }
}

/// Image file types we can detect.
#[derive(Debug, PartialEq, Eq)]
enum ImageType {
    Raw,
    Qcow2,
    CompositeDisk,
    AndroidSparse,
}

/// Detect image type by looking for magic bytes.
fn detect_image_type(file: &File) -> std::io::Result<ImageType> {
    const CDISK_MAGIC: &str = "composite_disk\x1d";
    const QCOW_MAGIC: u32 = 0x5146_49fb;
    const SPARSE_HEADER_MAGIC: u32 = 0xed26ff3a;

    let mut magic4 = [0u8; 4];
    match file.read_exact_at(&mut magic4[..], 0) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(ImageType::Raw),
        Err(e) => return Err(e),
    }
    if magic4 == QCOW_MAGIC.to_be_bytes() {
        return Ok(ImageType::Qcow2);
    }
    if magic4 == SPARSE_HEADER_MAGIC.to_le_bytes() {
        return Ok(ImageType::AndroidSparse);
    }

    let mut buf = [0u8; CDISK_MAGIC.len()];
    match file.read_exact_at(buf.as_bytes_mut(), 0) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(ImageType::Raw),
        Err(e) => return Err(e),
    }
    if buf == CDISK_MAGIC.as_bytes() {
        return Ok(ImageType::CompositeDisk);
    }

    Ok(ImageType::Raw)
}
