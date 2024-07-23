// Copyright 2024 The Android Open Source Project
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

//! CLI for converting file system to FDT

use clap::Parser;
use fsfdt::FsFdt;
use libfdt::Fdt;
use std::fs;
use std::path::PathBuf;

const FDT_MAX_SIZE: usize = 1_000_000_usize;

/// Option parser
#[derive(Parser, Debug)]
struct Opt {
    /// File system path (directory path) to parse from
    fs_path: PathBuf,

    /// FDT file path for writing
    fdt_file_path: PathBuf,
}

fn main() {
    let opt = Opt::parse();

    let mut data = vec![0_u8; FDT_MAX_SIZE];
    let fdt = Fdt::from_fs(&opt.fs_path, &mut data).unwrap();
    fdt.pack().unwrap();
    fs::write(&opt.fdt_file_path, fdt.as_slice()).unwrap();
}
