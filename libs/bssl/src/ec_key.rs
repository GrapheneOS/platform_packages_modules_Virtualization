// Copyright 2023, The Android Open Source Project
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

//! Contains struct and functions that wraps the API related to EC_KEY in
//! BoringSSL.

use crate::cbb::CbbFixed;
use crate::cbs::Cbs;
use crate::util::{check_int_result, to_call_failed_error};
use alloc::vec;
use alloc::vec::Vec;
use bssl_avf_error::{ApiName, Error, Result};
use bssl_sys::{
    i2d_ECDSA_SIG, BN_bin2bn, BN_bn2bin_padded, BN_clear_free, BN_new, CBB_flush, CBB_len,
    ECDSA_SIG_free, ECDSA_SIG_from_bytes, ECDSA_SIG_get0_r, ECDSA_SIG_get0_s, ECDSA_SIG_new,
    ECDSA_SIG_set0, ECDSA_sign, ECDSA_size, ECDSA_verify, EC_GROUP_get_curve_name,
    EC_GROUP_new_by_curve_name, EC_KEY_check_key, EC_KEY_free, EC_KEY_generate_key,
    EC_KEY_get0_group, EC_KEY_get0_public_key, EC_KEY_marshal_private_key,
    EC_KEY_new_by_curve_name, EC_KEY_parse_private_key, EC_KEY_set_public_key_affine_coordinates,
    EC_POINT_get_affine_coordinates, NID_X9_62_prime256v1, NID_secp384r1, BIGNUM, ECDSA_SIG,
    EC_GROUP, EC_KEY, EC_POINT,
};
use cbor_util::{get_label_value, get_label_value_as_bytes};
use ciborium::Value;
use core::mem;
use core::ptr::{self, NonNull};
use coset::{
    iana::{self, EnumI64},
    CborSerializable, CoseKey, CoseKeyBuilder, KeyType, Label,
};
use log::error;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const ES256_ALGO: iana::Algorithm = iana::Algorithm::ES256;
const P256_CURVE: iana::EllipticCurve = iana::EllipticCurve::P_256;
const P384_CURVE: iana::EllipticCurve = iana::EllipticCurve::P_384;
const P256_AFFINE_COORDINATE_SIZE: usize = 32;
const P384_AFFINE_COORDINATE_SIZE: usize = 48;

/// Wrapper of an `EC_KEY` object, representing a public or private EC key.
pub struct EcKey(pub(crate) NonNull<EC_KEY>);

impl Drop for EcKey {
    fn drop(&mut self) {
        // SAFETY: It is safe because the key has been allocated by BoringSSL and isn't
        // used after this.
        unsafe { EC_KEY_free(self.0.as_ptr()) }
    }
}

impl EcKey {
    /// Creates a new EC P-256 key pair.
    pub fn new_p256() -> Result<Self> {
        // SAFETY: The returned pointer is checked below.
        let ec_key = unsafe {
            EC_KEY_new_by_curve_name(NID_X9_62_prime256v1) // EC P-256 CURVE Nid
        };
        NonNull::new(ec_key)
            .map(Self)
            .ok_or_else(|| to_call_failed_error(ApiName::EC_KEY_new_by_curve_name))
    }

    /// Creates a new EC P-384 key pair.
    pub fn new_p384() -> Result<Self> {
        // SAFETY: The returned pointer is checked below.
        let ec_key = unsafe {
            EC_KEY_new_by_curve_name(NID_secp384r1) // EC P-384 CURVE Nid
        };
        NonNull::new(ec_key)
            .map(Self)
            .ok_or_else(|| to_call_failed_error(ApiName::EC_KEY_new_by_curve_name))
    }

    /// Constructs an `EcKey` instance from the provided COSE_Key encoded public key slice.
    pub fn from_cose_public_key_slice(cose_key: &[u8]) -> Result<Self> {
        let cose_key = CoseKey::from_slice(cose_key).map_err(|e| {
            error!("Failed to deserialize COSE_Key: {e:?}");
            Error::CoseKeyDecodingFailed
        })?;
        Self::from_cose_public_key(&cose_key)
    }

    /// Constructs an `EcKey` instance from the provided `COSE_Key`.
    ///
    /// The lifetime of the returned `EcKey` is not tied to the lifetime of the `cose_key`,
    /// because the affine coordinates stored in the `cose_key` are copied into the `EcKey`.
    ///
    /// Currently, only the EC P-256 and P-384 curves are supported.
    pub fn from_cose_public_key(cose_key: &CoseKey) -> Result<Self> {
        if cose_key.kty != KeyType::Assigned(iana::KeyType::EC2) {
            error!("Only EC2 keys are supported. Key type in the COSE Key: {:?}", cose_key.kty);
            return Err(Error::Unimplemented);
        }
        let ec_key =
            match get_label_value(cose_key, Label::Int(iana::Ec2KeyParameter::Crv.to_i64()))? {
                crv if crv == &Value::from(P256_CURVE.to_i64()) => EcKey::new_p256()?,
                crv if crv == &Value::from(P384_CURVE.to_i64()) => EcKey::new_p384()?,
                crv => {
                    error!(
                        "Only EC P-256 and P-384 curves are supported. \
                         Curve type in the COSE Key: {crv:?}"
                    );
                    return Err(Error::Unimplemented);
                }
            };
        let x = get_label_value_as_bytes(cose_key, Label::Int(iana::Ec2KeyParameter::X.to_i64()))?;
        let y = get_label_value_as_bytes(cose_key, Label::Int(iana::Ec2KeyParameter::Y.to_i64()))?;

        let group = ec_key.ec_group()?;
        group.check_affine_coordinate_size(x)?;
        group.check_affine_coordinate_size(y)?;

        let x = BigNum::from_slice(x)?;
        let y = BigNum::from_slice(y)?;

        // SAFETY: All the parameters are checked non-null and initialized.
        // The function only reads the coordinates x and y within their bounds.
        let ret = unsafe {
            EC_KEY_set_public_key_affine_coordinates(ec_key.0.as_ptr(), x.as_ref(), y.as_ref())
        };
        check_int_result(ret, ApiName::EC_KEY_set_public_key_affine_coordinates)?;
        ec_key.check_key()?;
        Ok(ec_key)
    }

    /// Performs several checks on the key. See BoringSSL doc for more details:
    ///
    /// https://commondatastorage.googleapis.com/chromium-boringssl-docs/ec_key.h.html#EC_KEY_check_key
    pub fn check_key(&self) -> Result<()> {
        // SAFETY: This function only reads the `EC_KEY` pointer, the non-null check is performed
        // within the function.
        let ret = unsafe { EC_KEY_check_key(self.0.as_ptr()) };
        check_int_result(ret, ApiName::EC_KEY_check_key)
    }

    /// Verifies the DER-encoded ECDSA `signature` of the `digest` with the current `EcKey`.
    ///
    /// Returns Ok(()) if the verification succeeds, otherwise an error will be returned.
    pub fn ecdsa_verify_der(&self, signature: &[u8], digest: &[u8]) -> Result<()> {
        // The `type` argument should be 0 as required in the BoringSSL spec.
        const TYPE: i32 = 0;

        // SAFETY: This function only reads the given data within its bounds.
        // The `EC_KEY` passed to this function has been initialized and checked non-null.
        let ret = unsafe {
            ECDSA_verify(
                TYPE,
                digest.as_ptr(),
                digest.len(),
                signature.as_ptr(),
                signature.len(),
                self.0.as_ptr(),
            )
        };
        check_int_result(ret, ApiName::ECDSA_verify)
    }

    /// Verifies the COSE-encoded (R | S, see RFC8152) ECDSA `signature` of the `digest` with the
    /// current `EcKey`.
    ///
    /// Returns Ok(()) if the verification succeeds, otherwise an error will be returned.
    pub fn ecdsa_verify_cose(&self, signature: &[u8], digest: &[u8]) -> Result<()> {
        let signature = ec_cose_signature_to_der(signature)?;
        self.ecdsa_verify_der(&signature, digest)
    }

    /// Signs the `digest` with the current `EcKey` using ECDSA.
    ///
    /// Returns the DER-encoded ECDSA signature.
    pub fn ecdsa_sign_der(&self, digest: &[u8]) -> Result<Vec<u8>> {
        // The `type` argument should be 0 as required in the BoringSSL spec.
        const TYPE: i32 = 0;

        let mut signature = vec![0u8; self.ecdsa_size()?];
        let mut signature_len = 0;
        // SAFETY: This function only reads the given data within its bounds.
        // The `EC_KEY` passed to this function has been initialized and checked non-null.
        let ret = unsafe {
            ECDSA_sign(
                TYPE,
                digest.as_ptr(),
                digest.len(),
                signature.as_mut_ptr(),
                &mut signature_len,
                self.0.as_ptr(),
            )
        };
        check_int_result(ret, ApiName::ECDSA_sign)?;
        if signature.len() < (signature_len as usize) {
            Err(to_call_failed_error(ApiName::ECDSA_sign))
        } else {
            signature.truncate(signature_len as usize);
            Ok(signature)
        }
    }

    /// Signs the `digest` with the current `EcKey` using ECDSA.
    ///
    /// Returns the COSE-encoded (R | S, see RFC8152) ECDSA signature.
    pub fn ecdsa_sign_cose(&self, digest: &[u8]) -> Result<Vec<u8>> {
        let signature = self.ecdsa_sign_der(digest)?;
        let coord_bytes = self.ec_group()?.affine_coordinate_size()?;
        ec_der_signature_to_cose(&signature, coord_bytes)
    }

    /// Returns the maximum size of an ECDSA signature using the current `EcKey`.
    fn ecdsa_size(&self) -> Result<usize> {
        // SAFETY: This function only reads the `EC_KEY` that has been initialized
        // and checked non-null when this instance is created.
        let size = unsafe { ECDSA_size(self.0.as_ptr()) };
        if size == 0 {
            Err(to_call_failed_error(ApiName::ECDSA_size))
        } else {
            Ok(size)
        }
    }

    /// Generates a random, private key, calculates the corresponding public key and stores both
    /// in the `EC_KEY`.
    pub fn generate_key(&mut self) -> Result<()> {
        // SAFETY: The non-null pointer is created with `EC_KEY_new_by_curve_name` and should
        // point to a valid `EC_KEY`.
        // The randomness is provided by `getentropy()` in `vmbase`.
        let ret = unsafe { EC_KEY_generate_key(self.0.as_ptr()) };
        check_int_result(ret, ApiName::EC_KEY_generate_key)
    }

    /// Returns the `CoseKey` for the public key.
    pub fn cose_public_key(&self) -> Result<CoseKey> {
        let (x, y) = self.public_key_coordinates()?;
        let curve = self.ec_group()?.coset_curve()?;
        let key = CoseKeyBuilder::new_ec2_pub_key(curve, x, y).algorithm(ES256_ALGO).build();
        Ok(key)
    }

    /// Returns the x and y coordinates of the public key.
    fn public_key_coordinates(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let ec_group = self.ec_group()?;
        let ec_point = self.public_key_ec_point()?;
        let mut x = BigNum::new()?;
        let mut y = BigNum::new()?;
        let ctx = ptr::null_mut();
        // SAFETY: All the parameters are checked non-null and initialized when needed.
        // The last parameter `ctx` is generated when needed inside the function.
        let ret = unsafe {
            EC_POINT_get_affine_coordinates(
                ec_group.as_ref(),
                ec_point,
                x.as_mut_ptr(),
                y.as_mut_ptr(),
                ctx,
            )
        };
        check_int_result(ret, ApiName::EC_POINT_get_affine_coordinates)?;
        let len = ec_group.affine_coordinate_size()?;
        Ok((x.to_padded_vec(len)?, y.to_padded_vec(len)?))
    }

    /// Returns a pointer to the public key point inside `EC_KEY`. The memory region pointed
    /// by the pointer is owned by the `EC_KEY`.
    fn public_key_ec_point(&self) -> Result<*const EC_POINT> {
        let ec_point =
           // SAFETY: It is safe since the key pair has been generated and stored in the
           // `EC_KEY` pointer.
           unsafe { EC_KEY_get0_public_key(self.0.as_ptr()) };
        if ec_point.is_null() {
            Err(to_call_failed_error(ApiName::EC_KEY_get0_public_key))
        } else {
            Ok(ec_point)
        }
    }

    /// Returns a pointer to the `EC_GROUP` object inside `EC_KEY`. The memory region pointed
    /// by the pointer is owned by the `EC_KEY`.
    fn ec_group(&self) -> Result<EcGroup<'_>> {
        let group =
           // SAFETY: It is safe since the key pair has been generated and stored in the
           // `EC_KEY` pointer.
           unsafe { EC_KEY_get0_group(self.0.as_ptr()) };
        if group.is_null() {
            Err(to_call_failed_error(ApiName::EC_KEY_get0_group))
        } else {
            // SAFETY: The pointer should be valid and points to an initialized `EC_GROUP`
            // since it is read from a valid `EC_KEY`.
            Ok(EcGroup(unsafe { &*group }))
        }
    }

    /// Constructs an `EcKey` instance from the provided DER-encoded ECPrivateKey slice.
    ///
    /// Currently, only the EC P-256 curve is supported.
    pub fn from_ec_private_key(der_encoded_ec_private_key: &[u8]) -> Result<Self> {
        // SAFETY: This function only returns a pointer to a static object, and the
        // return is checked below.
        let ec_group = unsafe {
            EC_GROUP_new_by_curve_name(NID_X9_62_prime256v1) // EC P-256 CURVE Nid
        };
        if ec_group.is_null() {
            return Err(to_call_failed_error(ApiName::EC_GROUP_new_by_curve_name));
        }
        let mut cbs = Cbs::new(der_encoded_ec_private_key);
        // SAFETY: The function only reads bytes from the buffer managed by the valid `CBS`
        // object, and the returned EC_KEY is checked.
        let ec_key = unsafe { EC_KEY_parse_private_key(cbs.as_mut(), ec_group) };

        let ec_key = NonNull::new(ec_key)
            .map(Self)
            .ok_or_else(|| to_call_failed_error(ApiName::EC_KEY_parse_private_key))?;
        ec_key.check_key()?;
        Ok(ec_key)
    }

    /// Returns the DER-encoded ECPrivateKey structure described in RFC 5915 Section 3:
    ///
    /// https://datatracker.ietf.org/doc/html/rfc5915#section-3
    pub fn ec_private_key(&self) -> Result<ZVec> {
        const CAPACITY: usize = 256;
        let mut buf = Zeroizing::new([0u8; CAPACITY]);
        let mut cbb = CbbFixed::new(buf.as_mut());
        let enc_flags = 0;
        let ret =
            // SAFETY: The function only write bytes to the buffer managed by the valid `CBB`
            // object, and the key has been allocated by BoringSSL.
            unsafe { EC_KEY_marshal_private_key(cbb.as_mut(), self.0.as_ptr(), enc_flags) };

        check_int_result(ret, ApiName::EC_KEY_marshal_private_key)?;
        // SAFETY: This is safe because the CBB pointer is a valid pointer initialized with
        // `CBB_init_fixed()`.
        check_int_result(unsafe { CBB_flush(cbb.as_mut()) }, ApiName::CBB_flush)?;
        // SAFETY: This is safe because the CBB pointer is initialized with `CBB_init_fixed()`,
        // and it has been flushed, thus it has no active children.
        let len = unsafe { CBB_len(cbb.as_ref()) };
        Ok(buf.get(0..len).ok_or_else(|| to_call_failed_error(ApiName::CBB_len))?.to_vec().into())
    }
}

/// Convert a COSE format (R | S) ECDSA signature to a DER-encoded form.
fn ec_cose_signature_to_der(signature: &[u8]) -> Result<Vec<u8>> {
    let mut ec_sig = EcSignature::new()?;
    ec_sig.load_from_cose(signature)?;
    ec_sig.to_der()
}

/// Convert a DER-encoded signature to COSE format (R | S).
fn ec_der_signature_to_cose(signature: &[u8], coord_bytes: usize) -> Result<Vec<u8>> {
    let ec_sig = EcSignature::new_from_der(signature)?;
    ec_sig.to_cose(coord_bytes)
}

/// Wrapper for an `ECDSA_SIG` object representing an EC signature.
struct EcSignature(NonNull<ECDSA_SIG>);

impl EcSignature {
    /// Allocate a signature object.
    fn new() -> Result<Self> {
        // SAFETY: We take ownership of the returned pointer if it is non-null.
        let signature = unsafe { ECDSA_SIG_new() };

        let signature =
            NonNull::new(signature).ok_or_else(|| to_call_failed_error(ApiName::ECDSA_SIG_new))?;
        Ok(Self(signature))
    }

    /// Populate the signature parameters from a COSE encoding (R | S).
    fn load_from_cose(&mut self, signature: &[u8]) -> Result<()> {
        let coord_bytes = signature.len() / 2;
        if signature.len() != 2 * coord_bytes {
            return Err(Error::InternalError);
        }
        let mut r = BigNum::from_slice(&signature[..coord_bytes])?;
        let mut s = BigNum::from_slice(&signature[coord_bytes..])?;

        check_int_result(
            // SAFETY: The ECDSA_SIG was properly allocated and not yet freed. We have ownership
            // of the two BigNums and they are not null.
            unsafe { ECDSA_SIG_set0(self.0.as_mut(), r.as_mut_ptr(), s.as_mut_ptr()) },
            ApiName::ECDSA_SIG_set0,
        )?;

        // On success, the ECDSA_SIG has taken ownership of the BigNums.
        mem::forget(r);
        mem::forget(s);

        Ok(())
    }

    fn to_cose(&self, coord_bytes: usize) -> Result<Vec<u8>> {
        let mut result = vec![0u8; coord_bytes.checked_mul(2).unwrap()];
        let (r_bytes, s_bytes) = result.split_at_mut(coord_bytes);

        // SAFETY: The ECDSA_SIG was properly allocated and not yet freed. Always returns a valid
        // non-null, non-owning pointer.
        let r = unsafe { ECDSA_SIG_get0_r(self.0.as_ptr()) };
        check_int_result(
            // SAFETY: The r pointer is known to be valid. Only writes within the destination
            // slice.
            unsafe { BN_bn2bin_padded(r_bytes.as_mut_ptr(), r_bytes.len(), r) },
            ApiName::BN_bn2bin_padded,
        )?;

        // SAFETY: The ECDSA_SIG was properly allocated and not yet freed. Always returns a valid
        // non-null, non-owning pointer.
        let s = unsafe { ECDSA_SIG_get0_s(self.0.as_ptr()) };
        check_int_result(
            // SAFETY: The r pointer is known to be valid. Only writes within the destination
            // slice.
            unsafe { BN_bn2bin_padded(s_bytes.as_mut_ptr(), s_bytes.len(), s) },
            ApiName::BN_bn2bin_padded,
        )?;

        Ok(result)
    }

    /// Populate the signature parameters from a DER encoding
    fn new_from_der(signature: &[u8]) -> Result<Self> {
        // SAFETY: Only reads within the bounds of the slice. Returns a pointer to a new ECDSA_SIG
        // which we take ownership of, or null on error which we check.
        let signature = unsafe { ECDSA_SIG_from_bytes(signature.as_ptr(), signature.len()) };

        let signature = NonNull::new(signature)
            .ok_or_else(|| to_call_failed_error(ApiName::ECDSA_SIG_from_bytes))?;
        Ok(Self(signature))
    }

    /// Return the signature encoded as DER.
    fn to_der(&self) -> Result<Vec<u8>> {
        // SAFETY: The ECDSA_SIG was properly allocated and not yet freed. Null is a valid
        // value for `outp`; no output is written.
        let len = unsafe { i2d_ECDSA_SIG(self.0.as_ptr(), ptr::null_mut()) };
        if len < 0 {
            return Err(to_call_failed_error(ApiName::i2d_ECDSA_SIG));
        }

        let mut buf = vec![0; len.try_into().map_err(|_| Error::InternalError)?];
        let outp = &mut buf.as_mut_ptr();
        // SAFETY: The ECDSA_SIG was properly allocated and not yet freed. `outp` is a non-null
        // pointer to a mutable buffer of the right size to which the result will be written.
        let final_len = unsafe { i2d_ECDSA_SIG(self.0.as_ptr(), outp) };
        if final_len < 0 {
            return Err(to_call_failed_error(ApiName::i2d_ECDSA_SIG));
        }
        // The input hasn't changed, so the length of the output shouldn't have. If it has we
        // already have potentially undefined behavior so panic.
        assert_eq!(
            len, final_len,
            "i2d_ECDSA_SIG returned inconsistent lengths: {len}, {final_len}"
        );

        Ok(buf)
    }
}

impl Drop for EcSignature {
    fn drop(&mut self) {
        // SAFETY: The pointer was allocated by `ECDSA_SIG_new`.
        unsafe { ECDSA_SIG_free(self.0.as_mut()) };
    }
}

/// Wrapper of an `EC_GROUP` reference.
struct EcGroup<'a>(&'a EC_GROUP);

impl<'a> EcGroup<'a> {
    /// Returns the NID that identifies the EC group of the key.
    fn curve_nid(&self) -> i32 {
        // SAFETY: It is safe since the inner pointer is valid and points to an initialized
        // instance of `EC_GROUP`.
        unsafe { EC_GROUP_get_curve_name(self.as_ref()) }
    }

    fn coset_curve(&self) -> Result<iana::EllipticCurve> {
        #[allow(non_upper_case_globals)]
        match self.curve_nid() {
            NID_X9_62_prime256v1 => Ok(P256_CURVE),
            NID_secp384r1 => Ok(P384_CURVE),
            name => {
                error!("Unsupported curve NID: {}", name);
                Err(Error::Unimplemented)
            }
        }
    }

    fn affine_coordinate_size(&self) -> Result<usize> {
        #[allow(non_upper_case_globals)]
        match self.curve_nid() {
            NID_X9_62_prime256v1 => Ok(P256_AFFINE_COORDINATE_SIZE),
            NID_secp384r1 => Ok(P384_AFFINE_COORDINATE_SIZE),
            name => {
                error!("Unsupported curve NID: {}", name);
                Err(Error::Unimplemented)
            }
        }
    }

    fn check_affine_coordinate_size(&self, coordinate: &[u8]) -> Result<()> {
        let expected_len = self.affine_coordinate_size()?;
        if expected_len == coordinate.len() {
            Ok(())
        } else {
            error!(
                "The size of the affine coordinate '{}' does not match the expected size '{}'",
                coordinate.len(),
                expected_len
            );
            Err(Error::CoseKeyDecodingFailed)
        }
    }
}

impl<'a> AsRef<EC_GROUP> for EcGroup<'a> {
    fn as_ref(&self) -> &EC_GROUP {
        self.0
    }
}

/// A u8 vector that is zeroed when dropped.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ZVec(Vec<u8>);

impl ZVec {
    /// Extracts a slice containing the entire vector.
    pub fn as_slice(&self) -> &[u8] {
        &self.0[..]
    }
}

impl From<Vec<u8>> for ZVec {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

struct BigNum(NonNull<BIGNUM>);

impl Drop for BigNum {
    fn drop(&mut self) {
        // SAFETY: The pointer has been created with `BN_new`.
        unsafe { BN_clear_free(self.as_mut_ptr()) }
    }
}

impl BigNum {
    fn from_slice(x: &[u8]) -> Result<Self> {
        // SAFETY: The function reads `x` within its bounds, and the returned
        // pointer is checked below.
        let bn = unsafe { BN_bin2bn(x.as_ptr(), x.len(), ptr::null_mut()) };
        NonNull::new(bn).map(Self).ok_or_else(|| to_call_failed_error(ApiName::BN_bin2bn))
    }

    fn new() -> Result<Self> {
        // SAFETY: The returned pointer is checked below.
        let bn = unsafe { BN_new() };
        NonNull::new(bn).map(Self).ok_or_else(|| to_call_failed_error(ApiName::BN_new))
    }

    /// Converts the `BigNum` to a big-endian integer. The integer is padded with leading zeros up
    /// to size `len`. The conversion fails if `len` is smaller than the size of the integer.
    fn to_padded_vec(&self, len: usize) -> Result<Vec<u8>> {
        let mut num = vec![0u8; len];
        // SAFETY: The `BIGNUM` pointer has been created with `BN_new`.
        let ret = unsafe { BN_bn2bin_padded(num.as_mut_ptr(), num.len(), self.0.as_ptr()) };
        check_int_result(ret, ApiName::BN_bn2bin_padded)?;
        Ok(num)
    }

    fn as_mut_ptr(&mut self) -> *mut BIGNUM {
        self.0.as_ptr()
    }
}

impl AsRef<BIGNUM> for BigNum {
    fn as_ref(&self) -> &BIGNUM {
        // SAFETY: The pointer is valid and points to an initialized instance of `BIGNUM`
        // when the instance was created.
        unsafe { self.0.as_ref() }
    }
}
