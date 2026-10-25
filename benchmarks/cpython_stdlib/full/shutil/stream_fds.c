/* A test-only adapter for CPython's already-open streams. Dup preserves the
 * kernel open-file description (including offset) without reopening or truncating
 * its pathname. Other file opens in the child retain their ordinary behavior. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

static int adapter(int dirfd, const char *path, int flags, mode_t mode) {
    const char *fd = NULL;
    if (!strcmp(path, "/imre-shutil-fixture/source-fd")) fd = getenv("SHUTIL_FIXTURE_SRC");
    if (!strcmp(path, "/imre-shutil-fixture/destination-fd")) fd = getenv("SHUTIL_FIXTURE_DST");
    if (fd) return fcntl(atoi(fd), F_DUPFD_CLOEXEC, 0);
    return (int)syscall(SYS_openat, dirfd, path, flags, mode);
}

#define OPEN_BODY(dir) do { \
    mode_t m = 0; \
    if ((flags & O_CREAT) || (flags & O_TMPFILE) == O_TMPFILE) { \
        va_list ap; va_start(ap, flags); m = va_arg(ap, mode_t); va_end(ap); \
    } \
    return adapter(dir, path, flags, m); \
} while (0)

int open(const char *path, int flags, ...) { OPEN_BODY(AT_FDCWD); }
int open64(const char *path, int flags, ...) { OPEN_BODY(AT_FDCWD); }
int openat(int dir, const char *path, int flags, ...) { OPEN_BODY(dir); }
int openat64(int dir, const char *path, int flags, ...) { OPEN_BODY(dir); }
