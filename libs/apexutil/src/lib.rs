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

//! Routines for handling APEX payload

use apex_manifest::apex_manifest::ApexManifest;
use protobuf::Message;
use std::fs::File;
use std::io::{self, Read};
use thiserror::Error;
use vbmeta::VbMetaImage;
use zip::result::ZipError;
use zip::ZipArchive;

const APEX_PUBKEY_ENTRY: &str = "apex_pubkey";
const APEX_PAYLOAD_ENTRY: &str = "apex_payload.img";
const APEX_MANIFEST_ENTRY: &str = "apex_manifest.pb";

/// Errors from parsing an APEX.
#[derive(Debug, Error)]
pub enum ApexParseError {
    /// There was an IO error.
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    /// The Zip archive was invalid.
    #[error("Cannot read zip archive: {0}")]
    InvalidZip(&'static str),
    /// An expected file was missing from the APEX.
    #[error("APEX doesn't contain {0}")]
    MissingFile(&'static str),
    /// There was no hashtree descriptor in the APEX payload's VBMeta image.
    #[error("Non-hashtree descriptor found in payload's VBMeta image")]
    DescriptorNotHashtree,
    /// There was an error parsing the APEX payload's VBMeta image.
    #[error("Could not parse payload's VBMeta image: {0}")]
    PayloadVbmetaError(#[from] vbmeta::VbMetaImageParseError),
    /// Data was missing from the VBMeta
    #[error("Data missing from VBMeta: {0}")]
    VbmetaMissingData(&'static str),
    /// An error occurred parsing the APEX manifest as a protobuf
    #[error("Error parsing manifest protobuf: {0}")]
    ManifestProtobufError(#[from] protobuf::Error),
}

/// Errors from verifying an APEX.
#[derive(Debug, Error)]
pub enum ApexVerificationError {
    /// There was an error parsing the APEX.
    #[error("Cannot parse APEX file")]
    ParseError(#[from] ApexParseError),
    /// There was an error validating the APEX payload's VBMeta image.
    #[error("Could not parse payload's VBMeta image")]
    PayloadVbmetaError(#[from] vbmeta::VbMetaImageVerificationError),
    /// The APEX payload was not verified with the apex_pubkey.
    #[error("APEX pubkey mismatch")]
    ApexPubkeyMismatch,
}

/// Information extracted from the APEX during AVB verification.
#[derive(Debug)]
pub struct ApexVerificationResult {
    /// The name of the APEX, from its manifest.
    pub name: Option<String>,
    /// The version of the APEX, from its manifest.
    pub version: Option<i64>,
    /// The public key that verifies the payload signature.
    pub public_key: Vec<u8>,
    /// The root digest of the payload hashtree.
    pub root_digest: Vec<u8>,
}

/// Verify APEX payload by AVB verification and return information about the APEX.
/// This verifies that the VBMeta is correctly signed by the public key specified in the APEX.
/// It doesn't verify that that is the correct key, nor does it verify that the payload matches
/// the signed root hash - that is handled by dm-verity once apexd has mounted the APEX.
pub fn verify(path: &str) -> Result<ApexVerificationResult, ApexVerificationError> {
    let apex_file = File::open(path).map_err(ApexParseError::Io)?;
    let ApexZipInfo { public_key, image_offset, image_size, manifest } =
        get_apex_zip_info(&apex_file)?;
    let vbmeta = VbMetaImage::verify_reader_region(apex_file, image_offset, image_size)?;
    let root_digest = find_root_digest(&vbmeta)?;
    let vbmeta_public_key =
        vbmeta.public_key().ok_or(ApexParseError::VbmetaMissingData("public key"))?;
    if vbmeta_public_key != public_key {
        return Err(ApexVerificationError::ApexPubkeyMismatch);
    }
    let (name, version) = if cfg!(dice_changes) {
        let ApexManifestInfo { name, version } = decode_manifest(&manifest)?;
        (Some(name), Some(version))
    } else {
        (None, None)
    };
    Ok(ApexVerificationResult { name, version, public_key, root_digest })
}

fn find_root_digest(vbmeta: &VbMetaImage) -> Result<Vec<u8>, ApexParseError> {
    // APEXs use the root digest from the first hashtree descriptor to describe the payload.
    for descriptor in vbmeta.descriptors()?.iter() {
        if let vbmeta::Descriptor::Hashtree(_) = descriptor {
            return Ok(descriptor.to_hashtree()?.root_digest().to_vec());
        }
    }
    Err(ApexParseError::DescriptorNotHashtree)
}

struct ApexZipInfo {
    public_key: Vec<u8>,
    image_offset: u64,
    image_size: u64,
    manifest: Vec<u8>,
}

fn get_apex_zip_info(apex_file: &File) -> Result<ApexZipInfo, ApexParseError> {
    let mut z = ZipArchive::new(apex_file).map_err(|err| from_zip_error(err, "?"))?;

    let mut public_key = Vec::new();
    z.by_name(APEX_PUBKEY_ENTRY)
        .map_err(|err| from_zip_error(err, APEX_PUBKEY_ENTRY))?
        .read_to_end(&mut public_key)?;

    let (image_offset, image_size) = z
        .by_name(APEX_PAYLOAD_ENTRY)
        .map(|f| (f.data_start(), f.size()))
        .map_err(|err| from_zip_error(err, APEX_PAYLOAD_ENTRY))?;

    let mut manifest = Vec::new();
    z.by_name(APEX_MANIFEST_ENTRY)
        .map_err(|err| from_zip_error(err, APEX_MANIFEST_ENTRY))?
        .read_to_end(&mut manifest)?;

    Ok(ApexZipInfo { public_key, image_offset, image_size, manifest })
}

struct ApexManifestInfo {
    name: String,
    version: i64,
}

fn decode_manifest(mut manifest: &[u8]) -> Result<ApexManifestInfo, ApexParseError> {
    let manifest = ApexManifest::parse_from_reader(&mut manifest)?;
    Ok(ApexManifestInfo { name: manifest.name, version: manifest.version })
}

fn from_zip_error(err: ZipError, name: &'static str) -> ApexParseError {
    match err {
        ZipError::Io(err) => ApexParseError::Io(err),
        ZipError::InvalidArchive(s) | ZipError::UnsupportedArchive(s) => {
            ApexParseError::InvalidZip(s)
        }
        ZipError::FileNotFound => ApexParseError::MissingFile(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apex_verification_returns_valid_result() {
        let res = verify("apex.apexd_test.apex").unwrap();
        let (expected_name, expected_version) = if cfg!(dice_changes) {
            (Some("com.android.apex.test_package"), Some(1))
        } else {
            (None, None)
        };
        assert_eq!(res.name.as_deref(), expected_name);
        assert_eq!(res.version, expected_version);
        // The expected hex values were generated when we ran the method the first time.
        assert_eq!(
            hex::encode(res.root_digest),
            "0cdeee606f2c9b468900e07d306384605684e6ca0b8bc6c51592808de9fd1c44"
        );
        assert_eq!(
            hex::encode(res.public_key),
            "\
            00001000963a5527aaf0145b3bb5f899a05034ccc76dafdd671dbf4e42c04df2eeba15\
            6c884816d7d08ef8d834d4adc27979afed9eaf406694d0d600f0b6d31e3ab85da47d27\
            9c223a1630e02332d920587617ea766a136057a3a3232a7c42f83fb3763e853be4026c\
            067524a95fcbfcc6caadfb553210bb5385f5adc5caeb0e3f6a9aa56af88d8899d962eb\
            807864feabeeacdd868697935fb4cc4843957e0d90ee4293c715c4e5b970e6545a17d1\
            735f814c7d4dbdeaac97275a84f292e3715c158d38eb00eebd010dd2fa56595c0e5627\
            06c7a94e566912f993e5e35c04b2a314d1bce1ceb10de6c50f8101ddb6ee993fc79959\
            2e79ee73b77741ee5c076c89343684344a6d080e5529a046d506d104bf32903e39c363\
            b020fee9d87e7c6ffdad120b630386e958416ac156bc2d7301836c79e926e8f185a640\
            be05135e17018c88dde02cd7bd49655e9e9dff7f965fb8e68217236c18d23b6d7e7632\
            184acb95b088598601c809d5e66c19f5e06b5e5ff1bbae7e3142959d9380db2d4a25c8\
            757975232ea311016e830703a6023b0986e885f2eda066517fce09f33f359b6ef7cc5a\
            2fdaced74257661bad184a653ea2d80d1af68de5821c06a472635f0276dc42d699f588\
            ea6c46189ca1ad544bbd4951a766bc4119b0ea671cb16556762721723bf1db47c83c76\
            a7cc2fd3b6029efec9908d9d4640294f6ea46f6e1a3195e9252c393e35698911a7c496\
            138dc2dd8d9dcb470ae1c6d2224d13b160fb3ae4bc235f6133c2ff5f9232fb89adfdba\
            48dcc47cf29a22cd47dcec0b1a179f352c9848a8e04ac37f35777a24312c821febc591\
            84c8cdefc88e50b4d6bc9530ca743f4284c9773677d38527e6e8020fe367f0f16a6c49\
            9a7f2da95ec6471f7382e5c0da98b531702cb55a560de7cafc7b6111aae0f896fb1fed\
            d4997a954c6c083ef1fd3bb13fef3f95022523fb1fbe7f4a49e12e54a5206f95daa316\
            ac009b7bee4039f769fd28033db6013df841c86d8345d44418fbc9f669e4ee3294b2ff\
            29d048f53d768c0a41f9a280f0229d9912e8b2fb734617a9947be973ed1dc7bdeac9e2\
            6028d59317098a44bacdb3b10ccde6ef02f7c94124461032a033701ce523b13142658c\
            265385198903ccf227ad5ae88ec31e586cd8f855641fd2646dba8053d0d0924f132505\
            8141f1c7433aa9686f48e3f3a972b56776eaf8bf22a740d1aea2ef473184d697de1dab\
            9b62a227611c7500b11dea2e5eb8051807c0d1f2fe032acfd7701c017e629f99c74de5\
            da4c2a542f17b9833beb14442aa7c2990b828473376ea03fdb4a650b88e821fe5026e8\
            ffb7002d095c9877ee3a98a4488ed3287e9be4942a223f4e32bc26c2ebd02eec20dc82\
            7493b44f4efaf9b2e175d4de2b07c32d6d359e234c9e50ef905ffa7f6907c313a3c9f4\
            40d1efd5ec7cbeef06dcfd649f4c8219ad"
        );
    }

    #[test]
    fn apex_no_manifest_fails_verification() {
        match verify("apex.apexd_test_v2_no_pb.apex").unwrap_err() {
            ApexVerificationError::ParseError(ApexParseError::MissingFile(_)) => (),
            e => panic!("Unexpected error {e}"),
        }
    }

    #[test]
    fn apex_signature_mismatch_fails_verification() {
        match verify("apex.apexd_test_wrong_public_key.apex").unwrap_err() {
            ApexVerificationError::ApexPubkeyMismatch => (),
            e => panic!("Unexpected error {e}"),
        }
    }
}
