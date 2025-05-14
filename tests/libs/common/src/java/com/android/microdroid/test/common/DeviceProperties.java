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

package com.android.microdroid.test.common;

import static java.util.Objects.requireNonNull;

/** This class can be used in both host tests and device tests to get the device properties. */
public final class DeviceProperties {

    /** PropertyGetter is used to get the property associated to a given key. */
    public interface PropertyGetter {
        String getProperty(String key) throws Exception;
    }

    private static final String KEY_VENDOR_DEVICE = "ro.product.vendor.device";
    private static final String KEY_BUILD_TYPE = "ro.build.type";
    private static final String KEY_PRODUCT_NAME = "ro.product.name";
    private static final String KEY_METRICS_TAG = "debug.hypervisor.metrics_tag";

    private static final String CUTTLEFISH_DEVICE_PREFIX = "vsoc_";
    private static final String CUTTLEFISH_ARM64_DEVICE_PREFIX = "vsoc_arm64";
    private static final String GOLDFISH_DEVICE_PREFIX = "emu64";
    private static final String GOLDFISH_ARM64_DEVICE_PREFIX = "emu64a";
    private static final String USER_BUILD_TYPE = "user";
    private static final String HWASAN_SUFFIX = "_hwasan";

    private final PropertyGetter mPropertyGetter;

    private DeviceProperties(PropertyGetter propertyGetter) {
        mPropertyGetter = requireNonNull(propertyGetter);
    }

    /** Creates a new instance of {@link DeviceProperties}. */
    public static DeviceProperties create(PropertyGetter propertyGetter) {
        return new DeviceProperties(propertyGetter);
    }

    /**
     * @return whether the device is a cuttlefish device.
     */
    public boolean isCuttlefish() {
        String vendorDeviceName = getProperty(KEY_VENDOR_DEVICE);
        return vendorDeviceName != null && vendorDeviceName.startsWith(CUTTLEFISH_DEVICE_PREFIX);
    }

    /**
     * @return whether the device is a cuttlefish device running on 64 bit Arm.
     */
    public boolean isCuttlefishArm64() {
        String vendorDeviceName = getProperty(KEY_VENDOR_DEVICE);
        return vendorDeviceName != null
                && vendorDeviceName.startsWith(CUTTLEFISH_ARM64_DEVICE_PREFIX);
    }

    /**
     * @return whether the device is a cuttlefish device.
     */
    public boolean isGoldfish() {
        String vendorDeviceName = getProperty(KEY_VENDOR_DEVICE);
        return vendorDeviceName != null && vendorDeviceName.startsWith(GOLDFISH_DEVICE_PREFIX);
    }

    /**
     * @return whether the device is a cuttlefish device running on 64 bit Arm.
     */
    public boolean isGoldfishArm64() {
        String vendorDeviceName = getProperty(KEY_VENDOR_DEVICE);
        return vendorDeviceName != null
                && vendorDeviceName.startsWith(GOLDFISH_ARM64_DEVICE_PREFIX);
    }

    /**
     * @return whether the build is HWASAN.
     */
    public boolean isHwasan() {
        String productName = getProperty(KEY_PRODUCT_NAME);
        return productName != null && productName.contains(HWASAN_SUFFIX);
    }

    /**
     * @return whether the device is user build.
     */
    public boolean isUserBuild() {
        return USER_BUILD_TYPE.equals(getProperty(KEY_BUILD_TYPE));
    }

    public String getMetricsTag() {
        return getProperty(KEY_METRICS_TAG);
    }

    private String getProperty(String key) {
        try {
            return mPropertyGetter.getProperty(key);
        } catch (Exception e) {
            throw new IllegalArgumentException("Cannot get property for the key: " + key, e);
        }
    }
}
