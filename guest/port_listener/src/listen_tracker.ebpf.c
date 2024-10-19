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
// src/platform2/vm_tools/port_listener/listen_tracker.ebpf.c

// bpf_helpers.h uses types defined here
#include <bpf/bpf_helpers.h>

#include "common.h"
#include "vmlinux.h"

// For some reason 6.1 doesn't include these symbols in the debug build
// so they don't get included in vmlinux.h. These features have existed since
// well before 6.1.
#define BPF_F_NO_PREALLOC (1U << 0)
#define BPF_ANY 0

struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 1 << 24);
} events SEC(".maps");

struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __type(key, struct sock*);
    __type(value, __u8);
    __uint(max_entries, 65535);
    __uint(map_flags, BPF_F_NO_PREALLOC);
} sockmap SEC(".maps");

const __u8 set_value = 0;

SEC("tp/sock/inet_sock_set_state")
int tracepoint_inet_sock_set_state(struct trace_event_raw_inet_sock_set_state* ctx) {
    // We don't support anything other than TCP.
    if (ctx->protocol != IPPROTO_TCP) {
        return 0;
    }
    struct sock* sk = (struct sock*)ctx->skaddr;
    // If we're transitioning away from LISTEN but we don't know about this
    // socket yet then don't report anything.
    if (ctx->oldstate == BPF_TCP_LISTEN && bpf_map_lookup_elem(&sockmap, &sk) == NULL) {
        return 0;
    }
    // If we aren't transitioning to or from TCP_LISTEN then we don't care.
    if (ctx->newstate != BPF_TCP_LISTEN && ctx->oldstate != BPF_TCP_LISTEN) {
        return 0;
    }

    struct event* ev;
    ev = bpf_ringbuf_reserve(&events, sizeof(*ev), 0);
    if (!ev) {
        return 0;
    }
    ev->port = ctx->sport;

    if (ctx->newstate == BPF_TCP_LISTEN) {
        bpf_map_update_elem(&sockmap, &sk, &set_value, BPF_ANY);
        ev->state = kPortListenerUp;
    }
    if (ctx->oldstate == BPF_TCP_LISTEN) {
        bpf_map_delete_elem(&sockmap, &sk);
        ev->state = kPortListenerDown;
    }
    bpf_ringbuf_submit(ev, 0);

    return 0;
}
