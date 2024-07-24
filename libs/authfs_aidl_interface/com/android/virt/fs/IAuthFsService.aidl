/*
 * Copyright (C) 2021 The Android Open Source Project
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

package com.android.virt.fs;

import com.android.virt.fs.AuthFsConfig;
import com.android.virt.fs.IAuthFs;

/** @hide */
interface IAuthFsService {
    const String AUTHFS_SERVICE_SOCKET_NAME = "authfs_service";

    /**
     * Creates an AuthFS mount given the config. Returns the binder object that represent the AuthFS
     * instance. The AuthFS setup is deleted once the lifetime of the returned binder object ends.
     */
    IAuthFs mount(in AuthFsConfig config);
}
