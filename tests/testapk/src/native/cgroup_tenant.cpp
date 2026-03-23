#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <vm_main.h>
#include <vm_payload.h>

#include <vector>

extern "C" int AVmPayload_main() {
    std::vector<uint8_t*> gMemoryHogs;
    size_t sizeBytes = 85 * 1024 * 1024;
    gMemoryHogs.reserve(sizeBytes / 4096);
    // Allocate page by page to handle OOM gracefully
    for (size_t i = 0; i < sizeBytes; i += 4096) {
        uint8_t* page = (uint8_t*)malloc(4096);
        if (!page) {
            break; // Stop allocating if we hit a hard OOM
        }
        page[0] = rand() % 256;
        gMemoryHogs.push_back(page);
    }
    AVmPayload_notifyPayloadReady();

    for (;;) {
        pause();
    }
    return 0;
}
