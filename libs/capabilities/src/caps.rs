/*
 * Copyright (C) 2022 The Android Open Source Project
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

//! A rust library wrapping the libcap functionality.

use anyhow::{bail, Result};
use cap_bindgen::{
    cap_clear_flag, cap_drop_bound, cap_flag_t, cap_free, cap_get_proc, cap_set_proc, cap_value_t,
    CAP_LAST_CAP,
};
use nix::errno::Errno;

/// Removes inheritable capabilities set for this process.
/// See: https://man7.org/linux/man-pages/man7/capabilities.7.html
pub fn drop_inheritable_caps() -> Result<()> {
    // SAFETY: we do not manipulate memory handled by libcap.
    unsafe {
        let caps = cap_get_proc();
        scopeguard::defer! {
            cap_free(caps as *mut std::os::raw::c_void);
        }
        if cap_clear_flag(caps, cap_flag_t::CAP_INHERITABLE) < 0 {
            let e = Errno::last();
            bail!("cap_clear_flag failed: {:?}", e)
        }
        if cap_set_proc(caps) < 0 {
            let e = Errno::last();
            bail!("cap_set_proc failed: {:?}", e)
        }
    }
    Ok(())
}

/// Drop bounding set capabitilies for this process.
/// See: https://man7.org/linux/man-pages/man7/capabilities.7.html
pub fn drop_bounding_set() -> Result<()> {
    let mut cap_id: cap_value_t = 0;
    while cap_id <= CAP_LAST_CAP.try_into().unwrap() {
        // SAFETY: we do not manipulate memory handled by libcap.
        unsafe {
            if cap_drop_bound(cap_id) == -1 {
                let e = Errno::last();
                bail!("cap_drop_bound failed for {}: {:?}", cap_id, e);
            }
        }
        cap_id += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Basic test to verify that calling drop_inheritable_caps doesn't fail
    #[test]
    fn test_drop_inheritable_caps() {
        let result = drop_inheritable_caps();
        assert!(result.is_ok(), "failed with: {result:?}")
    }
}
