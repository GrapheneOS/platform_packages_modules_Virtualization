/*
 * Copyright (c) 2024, The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! JNI bindings to call into `hwtrust` from Java.

use anyhow::Result;
use hwtrust::{dice, session::Session};
use jni::objects::{JByteArray, JClass};
use jni::sys::jboolean;
use jni::JNIEnv;
use log::{debug, error, info};

/// Validates the given DICE chain.
#[no_mangle]
pub extern "system" fn Java_com_android_microdroid_test_HwTrustJni_validateDiceChain(
    env: JNIEnv,
    _class: JClass,
    dice_chain: JByteArray,
    allow_any_mode: jboolean,
) -> jboolean {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("hwtrust_jni")
            .with_max_level(log::LevelFilter::Debug),
    );
    debug!("Starting the DICE chain validation ...");
    match validate_dice_chain(env, dice_chain, allow_any_mode) {
        Ok(_) => {
            info!("DICE chain validated successfully");
            true
        }
        Err(e) => {
            error!("Failed to validate DICE chain: {:?}", e);
            false
        }
    }
    .into()
}

fn validate_dice_chain(
    env: JNIEnv,
    jdice_chain: JByteArray,
    allow_any_mode: jboolean,
) -> Result<()> {
    let dice_chain = env.convert_byte_array(jdice_chain)?;
    let mut session = Session::default();
    session.set_allow_any_mode(allow_any_mode == jboolean::from(true));
    let _chain = dice::Chain::from_cbor(&session, &dice_chain)?;
    Ok(())
}
