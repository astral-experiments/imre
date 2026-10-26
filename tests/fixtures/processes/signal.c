#define _POSIX_C_SOURCE 200809L
#include <signal.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "dispositions") == 0) {
        struct sigaction pipe, size;
        if (sigaction(SIGPIPE, NULL, &pipe) || sigaction(SIGXFSZ, NULL, &size)) {
            return 1;
        }
        printf("%d %d\n", pipe.sa_handler == SIG_IGN, size.sa_handler == SIG_IGN);
        return 0;
    }
    raise(SIGPIPE);
    return 77;
}
