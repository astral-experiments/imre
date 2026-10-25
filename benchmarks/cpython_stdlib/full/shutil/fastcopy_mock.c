/* Relay the native syscall boundary to the original unittest.mock object. SCM_RIGHTS
 * preserves open descriptions when an upstream side effect itself calls the kernel. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/sendfile.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

static int fixture_source(int fd) {
    const char *p = getenv("SHUTIL_FAST_SOURCE");
    struct stat a, b;
    return p && !syscall(SYS_fstat, fd, &a) &&
           !syscall(SYS_newfstatat, AT_FDCWD, p, &b, 0) &&
           a.st_ino == b.st_ino && a.st_dev == b.st_dev;
}

/* response: zero runs the native syscall; one supplies a mocked result; two raises errno. */
static int request(int op, const int *fds, int n, int64_t off, int64_t size, int64_t *value) {
    const char *socket = getenv("SHUTIL_FAST_SOCKET");
    if (!socket) return 0;
    int64_t req[3] = {op, off, size}, reply[2];
    struct iovec io = {req, sizeof(req)};
    union { struct cmsghdr align; char buf[CMSG_SPACE(2 * sizeof(int))]; } control = {0};
    struct msghdr msg = {0};
    msg.msg_iov = &io;
    msg.msg_iovlen = 1;
    msg.msg_control = control.buf;
    msg.msg_controllen = CMSG_SPACE(n * sizeof(int));
    struct cmsghdr *c = CMSG_FIRSTHDR(&msg);
    c->cmsg_level = SOL_SOCKET; c->cmsg_type = SCM_RIGHTS;
    c->cmsg_len = CMSG_LEN(n * sizeof(int));
    memcpy(CMSG_DATA(c), fds, n * sizeof(int));
    int sock = atoi(socket);
    if (sendmsg(sock, &msg, MSG_NOSIGNAL) != sizeof(req) ||
        recv(sock, reply, sizeof(reply), MSG_WAITALL) != sizeof(reply)) _exit(96);
    *value = reply[1];
    if (reply[0] == 2) errno = (int)reply[1];
    return (int)reply[0];
}

ssize_t sendfile64(int out, int in, off64_t *offset, size_t size) {
    const int fds[2] = {in, out};
    int64_t n;
    switch (request(1, fds, 2, offset ? *offset : -1, size, &n)) {
    case 1: if (offset) *offset += n; return n;
    case 2: return -1;
    default: return syscall(SYS_sendfile, out, in, offset, size);
    }
}

ssize_t copy_file_range(int in, off64_t *in_off, int out, off64_t *out_off, size_t size, unsigned int flags) {
    const int fds[2] = {in, out};
    int64_t n;
    switch (request(0, fds, 2, out_off ? *out_off : -1, size, &n)) {
    case 1: if (in_off) *in_off += n; if (out_off) *out_off += n; return n;
    case 2: return -1;
    default: return syscall(SYS_copy_file_range, in, in_off, out, out_off, size, flags);
    }
}

int fstat(int fd, struct stat *stat) {
    int ret = syscall(SYS_fstat, fd, stat);
    if (!ret && getenv("SHUTIL_FAST_FSTAT") && fixture_source(fd)) {
        int64_t n;
        switch (request(2, &fd, 1, -1, 0, &n)) {
        case 1: stat->st_size = n; break;
        case 2: return -1;
        }
    }
    return ret;
}

ssize_t read(int fd, void *buffer, size_t size) {
    static int observed = 0;
    if (!observed && getenv("SHUTIL_FAST_GUARD_READ") && fixture_source(fd)) {
        int64_t n;
        observed = 1;
        request(3, &fd, 1, -1, size, &n);
    }
    return syscall(SYS_read, fd, buffer, size);
}
