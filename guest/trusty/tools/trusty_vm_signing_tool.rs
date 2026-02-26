//! trusty_vm_signing_tool extracts the .text section of an elf to re-signed it using avbtool.
//! It is assumed that the .text section represents a bin that has simply been pasted into an
//! elf.

use clap::Parser;
use elf::endian::AnyEndian;
use elf::section::SectionHeader;
use elf::ElfBytes;
use std::io::{Read, Seek, Write};
use std::path::PathBuf;
use std::process::Command;
use std::str;
use tempfile::NamedTempFile;

const SECTION_SIZE_OFFSET: usize = 32;

#[derive(Debug)]
enum Error {
    ExecutionFailed,
    InvocationFailed,
}

type Result<T> = std::result::Result<T, Error>;

/// Run a system command passed in as an slice of &str, the first one the
/// command, the rest - the command line parameters.
///
/// Explicitly check command execution return code, if there is no execution
/// error return the stdout of the command as String.
fn run_command(cmd: &[&str]) -> Result<String> {
    let output = Command::new(cmd[0]).args(&cmd[1..]).output().map_err(|_| {
        eprintln!("Failed to execute command {}", cmd.join(" "));
        Error::InvocationFailed
    })?;
    if !output.status.success() {
        eprintln!("Command \"{}\" returned {}", cmd.join(" "), output.status.code().unwrap());
        Err(Error::ExecutionFailed)
    } else {
        Ok(String::from_utf8(output.stdout).unwrap())
    }
}

fn main() -> Result<()> {
    let config = Config::parse();

    let path = config.elf_file;
    let mut file_data = std::fs::read(&path).expect("Could not read file.");
    let slice = file_data.as_mut_slice();
    let file = ElfBytes::<AnyEndian>::minimal_parse(slice).expect("Failed to parse elf.");

    let header_size = file.ehdr.e_shentsize as usize;
    let header_start = file.ehdr.e_shoff as usize;
    let header_count = file.ehdr.e_shnum as usize;

    let hdr: SectionHeader = file
        .section_header_by_name(".text")
        .expect(".text section not found")
        .expect(".text section not found");

    let mut maybe_size = header_start + SECTION_SIZE_OFFSET;
    let mut found = false;
    for _ in 0..header_count {
        let sz = u64::from_le_bytes(slice[maybe_size..maybe_size + 8].try_into().unwrap());
        if sz == hdr.sh_size {
            found = true;
            break;
        }
        maybe_size += header_size;
    }
    assert!(found, "Failed to find section header");

    let offset = hdr.sh_offset as usize;
    let size = hdr.sh_size as usize;

    let bin = &mut slice[offset..offset + size];
    let mut temp = NamedTempFile::new().expect("Failed to create temp file.");

    temp.write_all(bin).expect("Failed to write to temp file.");
    temp.flush().expect("Failed to flush temp file.");

    let mut cmd = vec![
        config.avbtool.to_str().unwrap(),
        "resign_image",
        "--image",
        temp.path().to_str().unwrap(),
        "--key",
        config.key.to_str().unwrap(),
        "--algorithm",
        &config.algorithm,
    ];
    if let Some(ref helper) = config.signing_helper_with_files {
        cmd.push("--signing_helper_with_files");
        cmd.push(helper);
    }
    let out = run_command(&cmd).expect("Failed to run command.");
    println!("{}", out);

    temp.rewind().expect("Failed to rewind file.");
    let mut signed = Vec::new();
    temp.read_to_end(&mut signed).expect("Failed to read signed bin.");

    bin[..signed.len()].copy_from_slice(&signed);
    if signed.len() < bin.len() {
        bin[signed.len()..].fill(0);
    }

    let path_tmp = temp.keep().unwrap();
    println!("Intermediate file: {:?}", path_tmp);
    let sz_slice = &mut slice[maybe_size..maybe_size + 8];
    sz_slice.copy_from_slice(&(signed.len() as u64).to_le_bytes());

    std::fs::write(path, slice).expect("Failed to overwrite target file.");

    Ok(())
}

#[derive(Debug, Parser)]
struct Config {
    #[clap(long, required = true, help = "The elf file to sign")]
    elf_file: PathBuf,

    #[clap(long, required = true, help = "File path for avbtool")]
    avbtool: PathBuf,

    #[clap(long, required = true, help = "The signing key to use")]
    key: PathBuf,

    #[clap(long, required = true, help = "The algorithm to use with key")]
    algorithm: String,

    #[clap(long("signing_helper_with_files"), help = "Signing helper script to use")]
    signing_helper_with_files: Option<String>,
}
