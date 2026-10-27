#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <sys/stat.h>
#include <unistd.h>

ssize_t getrandom(void *data, size_t size, unsigned int flags) {
    const char *test = getenv("IMRE_SECRETS_TEST_SOURCE");
    /* Leave separate nonblocking/hash-table entropy requests untouched. */
    if (test && size && flags == 0) {
        if (strcmp(test, "fail") == 0) {
            errno = EIO;
            return -1;
        }
        if (size == sizeof(uint64_t)) {
            static unsigned int draws;
            uint64_t sample = draws++ == 0 ? 0 : 8;
            memcpy(data, &sample, size);
            write(STDERR_FILENO, "draw\n", 5);
            return size;
        }
    }
    ssize_t (*next)(void *, size_t, unsigned int) = dlsym(RTLD_NEXT, "getrandom");
    return next(data, size, flags);
}

/* Some CPython builds draw the same byte by reading /dev/urandom. */
ssize_t read(int fd, void *data, size_t size) {
    const char *test = getenv("IMRE_SECRETS_TEST_SOURCE");
    if (size == 1 && test && strcmp(test, "fail") == 0) {
        struct stat opened, source;
        if (fstat(fd, &opened) == 0 && S_ISCHR(opened.st_mode)
            && stat("/dev/urandom", &source) == 0 && opened.st_rdev == source.st_rdev) {
            errno = EIO;
            return -1;
        }
    }
    ssize_t (*next)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    return next(fd, data, size);
}
