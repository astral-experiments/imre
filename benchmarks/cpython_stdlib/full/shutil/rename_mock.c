/* Test transport for test_shutil.mock_rename: deliver the real native syscall
 * operands to the upstream Python mock and return its errno to the caller. */
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int transfer(int fd, void *data, size_t length, int sending) {
    char *p = data;
    while (length) {
        ssize_t n = sending ? write(fd, p, length) : read(fd, p, length);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return -1;
        p += n;
        length -= (size_t)n;
    }
    return 0;
}

int rename(const char *from, const char *to) {
    const char *socket = getenv("SHUTIL_TEST_RENAME_FD");
    int reply, fd = socket ? atoi(socket) : -1;
    if (fd < 0 || transfer(fd, (void *)from, strlen(from) + 1, 1) ||
        transfer(fd, (void *)to, strlen(to) + 1, 1) ||
        transfer(fd, &reply, sizeof(reply), 0)) {
        errno = EIO;
        return -1;
    }
    errno = reply;
    return reply ? -1 : 0;
}

int renameat(int oldfd, const char *from, int newfd, const char *to) {
    if (oldfd == AT_FDCWD && newfd == AT_FDCWD) return rename(from, to);
    errno = ENOSYS;
    return -1;
}
