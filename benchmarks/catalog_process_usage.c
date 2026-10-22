/* Measure one fresh Linux process without inheriting the Python driver's heap. */
#define _DEFAULT_SOURCE
#include <errno.h>
#include <inttypes.h>
#include <spawn.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <time.h>

extern char **environ;

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    struct timespec start, end;
    if (clock_gettime(CLOCK_MONOTONIC, &start) < 0) return 2;
    pid_t child;
    int error = posix_spawn(&child, argv[2], NULL, NULL, &argv[2], environ);
    if (error) {
        fprintf(stderr, "posix_spawn: %s\n", strerror(error));
        return 2;
    }
    int status;
    struct rusage usage;
    while (wait4(child, &status, 0, &usage) < 0) {
        if (errno != EINTR) return 2;
    }
    if (clock_gettime(CLOCK_MONOTONIC, &end) < 0) return 2;
    int64_t elapsed = (int64_t)(end.tv_sec - start.tv_sec) * INT64_C(1000000000)
                      + end.tv_nsec - start.tv_nsec;
    FILE *metrics = fopen(argv[1], "w");
    if (!metrics) return 2;
    if (fprintf(metrics, "%" PRId64 " %ld\n", elapsed, usage.ru_maxrss) < 0) return 2;
    if (fclose(metrics)) return 2;
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}
