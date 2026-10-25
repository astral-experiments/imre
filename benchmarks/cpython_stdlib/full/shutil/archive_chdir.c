/* The upstream no_chdir mock must also observe directory changes by the native child. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

int chdir(const char *path) {
    const char *report = getenv("IMRE_SHUTIL_TEST_CHDIR_FD");
    if (!report) return syscall(SYS_chdir, path);
    /* A NUL terminates each attempted path without losing spaces or newlines. */
    size_t size = strlen(path) + 1;
    if (write(atoi(report), path, size) != (ssize_t)size) _exit(125);
    if (getenv("IMRE_SHUTIL_TRACE_CHDIR")) return syscall(SYS_chdir, path);
    errno = EPERM;
    return -1;
}
