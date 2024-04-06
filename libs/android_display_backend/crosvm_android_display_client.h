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

extern "C" {

typedef void (*android_display_log_callback_type)(const char* message);

static void android_display_log_callback_stub(const char* message) {
    (void)message;
}

struct android_display_context {
    uint32_t test;
};

__attribute__((visibility("default"))) struct android_display_context*
create_android_display_context(const char* name, size_t name_len,
                               android_display_log_callback_type error_callback);

__attribute__((visibility("default"))) void destroy_android_display_context(
        android_display_log_callback_type error_callback, struct android_display_context* ctx);

} // extern C
