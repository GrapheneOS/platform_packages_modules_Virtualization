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
// src/platform2/vm_tools/port_listener/main.cc

#include <bpf/libbpf.h>
#include <bpf/libbpf_legacy.h>
#include <glog/logging.h>
#include <linux/vm_sockets.h> // Needs to come after sys/socket.h
#include <sys/socket.h>

#include <memory>
#include <unordered_map>

#include "common.h"
#include "listen_tracker.skel.h"

typedef std::unordered_map<int, int> port_usage_map;

namespace port_listener {
namespace {

int HandleEvent(void* ctx, void* const data, size_t size) {
    port_usage_map* map = reinterpret_cast<port_usage_map*>(ctx);
    const struct event* ev = (struct event*)data;

    switch (ev->state) {
        case kPortListenerUp:
            (*map)[ev->port]++;
            break;

        case kPortListenerDown:
            if ((*map)[ev->port] > 0) {
                (*map)[ev->port]--;
            } else {
                LOG(INFO) << "Received down event while port count was 0; ignoring";
            }

            break;

        default:
            LOG(ERROR) << "Unknown event state " << ev->state;
    }

    LOG(INFO) << "Listen event: port=" << ev->port << " state=" << ev->state;

    return 0;
}

typedef std::unique_ptr<struct ring_buffer, decltype(&ring_buffer__free)> ring_buffer_ptr;
typedef std::unique_ptr<listen_tracker_ebpf, decltype(&listen_tracker_ebpf__destroy)>
        listen_tracker_ptr;

// BPFProgram tracks the state and resources of the listen_tracker BPF program.
class BPFProgram {
public:
    // Default movable but not copyable.
    BPFProgram(BPFProgram&& other) = default;
    BPFProgram(const BPFProgram& other) = delete;
    BPFProgram& operator=(BPFProgram&& other) = default;
    BPFProgram& operator=(const BPFProgram& other) = delete;

    // Load loads the listen_tracker BPF program and prepares it for polling. On
    // error nullptr is returned.
    static std::unique_ptr<BPFProgram> Load() {
        auto* skel = listen_tracker_ebpf__open();
        if (!skel) {
            PLOG(ERROR) << "Failed to open listen_tracker BPF skeleton";
            return nullptr;
        }
        listen_tracker_ptr skeleton(skel, listen_tracker_ebpf__destroy);

        int err = listen_tracker_ebpf__load(skeleton.get());
        if (err) {
            PLOG(ERROR) << "Failed to load listen_tracker BPF program";
            return nullptr;
        }

        auto map = std::make_unique<port_usage_map>();
        auto* rb = ring_buffer__new(bpf_map__fd(skel->maps.events), HandleEvent, map.get(), NULL);
        if (!rb) {
            PLOG(ERROR) << "Failed to open ring buffer for listen_tracker";
            return nullptr;
        }
        ring_buffer_ptr ringbuf(rb, ring_buffer__free);

        err = listen_tracker_ebpf__attach(skeleton.get());
        if (err) {
            PLOG(ERROR) << "Failed to attach listen_tracker";
            return nullptr;
        }

        return std::unique_ptr<BPFProgram>(
                new BPFProgram(std::move(skeleton), std::move(ringbuf), std::move(map)));
    }

    // Poll waits for the listen_tracker BPF program to post a new event to the
    // ring buffer. BPFProgram handles integrating this new event into the
    // port_usage map and callers should consult port_usage() after Poll returns
    // for the latest data.
    const bool Poll() {
        int err = ring_buffer__poll(rb_.get(), -1);
        if (err < 0) {
            LOG(ERROR) << "Error polling ring buffer ret=" << err;
            return false;
        }

        return true;
    }

    const port_usage_map& port_usage() { return *port_usage_; }

private:
    BPFProgram(listen_tracker_ptr&& skeleton, ring_buffer_ptr&& rb,
               std::unique_ptr<port_usage_map>&& port_usage)
          : skeleton_(std::move(skeleton)),
            rb_(std::move(rb)),
            port_usage_(std::move(port_usage)) {}

    listen_tracker_ptr skeleton_;
    ring_buffer_ptr rb_;
    std::unique_ptr<port_usage_map> port_usage_;
};

} // namespace
} // namespace port_listener

int main(int argc, char** argv) {
    google::InitGoogleLogging(argv[0]);
    libbpf_set_strict_mode(LIBBPF_STRICT_ALL);

    // Load our BPF program.
    auto program = port_listener::BPFProgram::Load();
    if (program == nullptr) {
        LOG(ERROR) << "Failed to load BPF program";
        return EXIT_FAILURE;
    }

    // main loop: poll for listen updates
    for (;;) {
        if (!program->Poll()) {
            LOG(ERROR) << "Failure while polling BPF program";
            return EXIT_FAILURE;
        }
        // port_usage will be updated with the latest usage data

        for (auto it : program->port_usage()) {
            if (it.second <= 0) {
                continue;
            }
            // TODO(b/340126051): Add listening TCP4 ports.
        }
        // TODO(b/340126051): Notify port information to the guest agent.
    }
}
