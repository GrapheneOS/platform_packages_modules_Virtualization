// Copyright 2024 The Android Open Source Project
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

// Copied from ChromiumOS with relicensing:
// src/platform2/vm_tools/port_listener/common.h

#ifndef VM_TOOLS_PORT_LISTENER_COMMON_H_
#define VM_TOOLS_PORT_LISTENER_COMMON_H_

enum State {
    kPortListenerUp,
    kPortListenerDown,
};

struct event {
    enum State state;
    uint16_t port;
};

#endif // VM_TOOLS_PORT_LISTENER_COMMON_H_
