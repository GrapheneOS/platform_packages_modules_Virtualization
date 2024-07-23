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

//! Attach/Detach bootconfigs to initrd image
use anyhow::{bail, Result};
use clap::Parser;
use std::cmp::min;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::path::PathBuf;

const FOOTER_ALIGNMENT: usize = 4;
const ZEROS: [u8; 4] = [0u8; 4_usize];
const BOOTCONFIG_MAGIC: &str = "#BOOTCONFIG\n";
// Footer includes [size(le32)][checksum(le32)][#BOOTCONFIG\n] at the end of bootconfigs.
const INITRD_FOOTER_LEN: usize = 2 * std::mem::size_of::<u32>() + BOOTCONFIG_MAGIC.len();

#[derive(Parser, Debug)]
enum Opt {
    /// Append bootconfig(s) to initrd image
    Attach {
        /// Initrd (without bootconfigs) <- Input
        initrd: PathBuf,
        /// Bootconfigs <- Input
        bootconfigs: Vec<PathBuf>,
        /// Initrd (with bootconfigs) <- Output
        #[clap(long = "output")]
        output: PathBuf,
    },

    /// Detach the initrd & bootconfigs - this is required for cases when we update
    /// bootconfigs in sign_virt_apex
    Detach {
        /// Initrd (with bootconfigs) <- Input
        initrd_with_bootconfigs: PathBuf,
        /// Initrd (without bootconfigs) <- Output
        initrd: PathBuf,
        /// Bootconfigs <- Output
        bootconfigs: PathBuf,
    },
}

fn get_checksum(file_path: &PathBuf) -> Result<u32> {
    File::open(file_path)?.bytes().map(|x| Ok(x? as u32)).sum()
}

// Copy n bytes of file_in to file_out. Note: copying starts from the current cursors of files.
// On successful return, the files' cursors would have moved forward by k bytes.
fn copyfile2file(file_in: &mut File, file_out: &mut File, n: usize) -> Result<()> {
    let mut buf = vec![0; 1024];
    let mut copied: usize = 0;
    while copied < n {
        let k = min(n - copied, buf.len());
        file_in.read_exact(&mut buf[..k])?;
        file_out.write_all(&buf[..k])?;
        copied += k;
    }
    Ok(())
}

// Note: attaching & then detaching bootconfigs can lead to extra padding in bootconfigs
fn detach_bootconfig(initrd_bc: PathBuf, initrd: PathBuf, bootconfig: PathBuf) -> Result<()> {
    let mut initrd_bc = File::open(initrd_bc)?;
    let mut bootconfig = File::create(bootconfig)?;
    let mut initrd = File::create(initrd)?;
    let initrd_bc_size: usize = initrd_bc.metadata()?.len().try_into()?;

    initrd_bc.seek(SeekFrom::End(-(BOOTCONFIG_MAGIC.len() as i64)))?;
    let mut magic_buf = [0; BOOTCONFIG_MAGIC.len()];
    initrd_bc.read_exact(&mut magic_buf)?;
    if magic_buf != BOOTCONFIG_MAGIC.as_bytes() {
        bail!("BOOTCONFIG_MAGIC not found in initrd. Bootconfigs might not be attached correctly");
    }
    let mut size_buf = [0; size_of::<u32>()];
    initrd_bc.seek(SeekFrom::End(-(INITRD_FOOTER_LEN as i64)))?;
    initrd_bc.read_exact(&mut size_buf)?;
    let bc_size: usize = u32::from_le_bytes(size_buf) as usize;

    let initrd_size: usize = initrd_bc_size - bc_size - INITRD_FOOTER_LEN;

    initrd_bc.rewind()?;
    copyfile2file(&mut initrd_bc, &mut initrd, initrd_size)?;
    copyfile2file(&mut initrd_bc, &mut bootconfig, bc_size)?;
    Ok(())
}

// Bootconfig is attached to the initrd in the following way:
// [initrd][bootconfig][padding][size(le32)][checksum(le32)][#BOOTCONFIG\n]
fn attach_bootconfig(initrd: PathBuf, bootconfigs: Vec<PathBuf>, output: PathBuf) -> Result<()> {
    let mut output_file = File::create(output)?;
    let mut initrd_file = File::open(initrd)?;
    let initrd_size: usize = initrd_file.metadata()?.len().try_into()?;
    let mut bootconfig_size: usize = 0;
    let mut checksum: u32 = 0;

    std::io::copy(&mut initrd_file, &mut output_file)?;
    for bootconfig in bootconfigs {
        let mut bootconfig_file = File::open(&bootconfig)?;
        std::io::copy(&mut bootconfig_file, &mut output_file)?;
        bootconfig_size += bootconfig_file.metadata()?.len() as usize;
        checksum += get_checksum(&bootconfig)?;
    }

    let padding_size: usize =
        (FOOTER_ALIGNMENT - (initrd_size + bootconfig_size) % FOOTER_ALIGNMENT) % FOOTER_ALIGNMENT;
    output_file.write_all(&ZEROS[..padding_size])?;
    output_file.write_all(&((padding_size + bootconfig_size) as u32).to_le_bytes())?;
    output_file.write_all(&checksum.to_le_bytes())?;
    output_file.write_all(BOOTCONFIG_MAGIC.as_bytes())?;
    output_file.flush()?;
    Ok(())
}

fn try_main() -> Result<()> {
    let args = Opt::parse();
    match args {
        Opt::Attach { initrd, bootconfigs, output } => {
            attach_bootconfig(initrd, bootconfigs, output)?
        }
        Opt::Detach { initrd_with_bootconfigs, initrd, bootconfigs } => {
            detach_bootconfig(initrd_with_bootconfigs, initrd, bootconfigs)?
        }
    };
    Ok(())
}

fn main() {
    try_main().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify_args() {
        // Check that the command parsing has been configured in a valid way.
        Opt::command().debug_assert();
    }
}
