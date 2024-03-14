// Copyright 2024, The Android Open Source Project
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

//! Derives microdroid vendor dice node.

use anyhow::Error;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    /// Path to the dice driver (e.g. /dev/open-dice0)
    #[arg(long)]
    dice_driver: PathBuf,
    /// Path to the microdroid-vendor.img disk image.
    #[arg(long)]
    microdroid_vendor_disk_image: PathBuf,
    /// File to save resulting dice chain to.
    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<(), Error> {
    let args = Args::parse();
    eprintln!("{:?} {:?} {:?}", args.dice_driver, args.microdroid_vendor_disk_image, args.output);
    Ok(())
}
