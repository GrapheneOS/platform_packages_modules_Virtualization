/*
 * Copyright 2022 The Android Open Source Project
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

#pragma once

#ifdef __cplusplus
extern "C" {
typedef int AVmPayload_main_t();
AVmPayload_main_t AVmPayload_main;
}
#else
typedef int AVmPayload_main_t(void);

/**
 * Entry point for the VM payload. This function must be implemented by the
 * payload binary, and is called by Microdroid to start the payload inside the
 * VM.
 *
 * When the function returns the VM will be shut down.  If the host app has set
 * a `VirtualMachineCallback` for the VM, its `onPayloadFinished` method will be
 * called with the VM's exit code.
 *
 * \return the exit code of the VM.
 */
extern int AVmPayload_main(void);
#endif
