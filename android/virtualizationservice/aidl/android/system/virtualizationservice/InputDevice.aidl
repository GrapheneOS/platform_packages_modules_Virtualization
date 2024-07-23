/*
 * Copyright 2024 The Android Open Source Project
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
package android.system.virtualizationservice;

// Refer to https://crosvm.dev/book/devices/input.html
union InputDevice {
    // Add a single-touch touchscreen virtio-input device.
    parcelable SingleTouch {
        ParcelFileDescriptor pfd;
        // Default values come from https://crosvm.dev/book/devices/input.html#single-touch
        int width = 1280;
        int height = 1080;
        @utf8InCpp String name = "";
    }
    // Passes an event device node into the VM. The device will be grabbed (unusable from the host)
    // and made available to the guest with the same configuration it shows on the host.
    parcelable EvDev {
        ParcelFileDescriptor pfd;
    }
    // Keyboard input
    parcelable Keyboard {
        ParcelFileDescriptor pfd;
    }
    // Mouse input
    parcelable Mouse {
        ParcelFileDescriptor pfd;
    }

    // Switches input
    parcelable Switches {
        ParcelFileDescriptor pfd;
    }

    parcelable Trackpad {
        ParcelFileDescriptor pfd;
        // Default values come from https://crosvm.dev/book/devices/input.html#trackpad
        int width = 1280;
        int height = 1080;
        @utf8InCpp String name = "";
    }

    parcelable MultiTouch {
        ParcelFileDescriptor pfd;
        // Default values come from https://crosvm.dev/book/devices/input.html#multi-touch
        int width = 1280;
        int height = 1080;
        @utf8InCpp String name = "";
    }

    SingleTouch singleTouch;
    EvDev evDev;
    Keyboard keyboard;
    Mouse mouse;
    Switches switches;
    Trackpad trackpad;
    MultiTouch multiTouch;
}
