#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <vm_main.h>
#include <vm_payload.h>

static void signalHandler(int signum) {
    if (signum == SIGTERM) {
        printf("SIGTERM received.\n");
        // Do some clean-up task here.
        _exit(1);
    } else {
        printf("Unexpected signal(%d) received.\n", signum);
    }
}

static void setupSignalHandler() {
    struct sigaction sa;
    sa.sa_handler = &signalHandler;
    sa.sa_flags = SA_RESTART;
    if (sigaction(SIGTERM, &sa, nullptr) == -1) {
        printf("Failed to set signal handler: %s\n", strerror(errno));
    }
}

extern "C" int AVmPayload_main() {
    // disable buffering to communicate seamlessly
    setvbuf(stdin, nullptr, _IONBF, 0);
    setvbuf(stdout, nullptr, _IONBF, 0);
    setvbuf(stderr, nullptr, _IONBF, 0);

    setupSignalHandler();

    printf("Hello Microdroid\n");

    AVmPayload_notifyPayloadReady();

    // Wait forever to allow developer to interact with Microdroid shell
    for (;;) {
        pause();
    }

    return 0;
}
