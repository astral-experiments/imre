/* Linux test process only. Record which syscall paths really experienced a fault. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#define NEXT(fn) ((__typeof__(&fn))dlsym(RTLD_NEXT, #fn))
#define TRACE(s) ((void)write(STDERR_FILENO, s "\n", sizeof(s)))
#define INTERRUPT(s) do { TRACE(s); errno = EINTR; return -1; } while (0)

static int mode(const char *name) {
    const char *value = getenv("IMRE_SOCKET_FAULT");
    return value && strcmp(value, name) == 0;
}

static int close_fd = -1;
static int closing;

int socket(int family, int type, int proto) {
    int fd = NEXT(socket)(family, type, proto);
    if (fd >= 0 && family == AF_INET && close_fd < 0 && (mode("close") || mode("close-reset")))
        close_fd = fd;
    return fd;
}

int close(int fd) {
    if (fd != close_fd || fd < 0)
        return NEXT(close)(fd);
    if (closing++) {
        TRACE("close:twice");
        return NEXT(close)(fd);
    }
    /* Linux may release the number even on error. Make reuse observable. */
    if (NEXT(close)(fd) != 0)
        _exit(90);
    int other = open("/dev/null", O_RDONLY | O_CLOEXEC);
    if (other < 0 || (other != fd && dup2(other, fd) < 0))
        _exit(91);
    if (other != fd)
        NEXT(close)(other);
    TRACE("close:reused");
    errno = mode("close-reset") ? ECONNRESET : EINTR;
    return -1;
}

static int connecting_fd = -1;
static int polled;
static int checked_error;

int connect(int fd, const struct sockaddr *addr, socklen_t len) {
    if (addr->sa_family != AF_INET || !(mode("io") || mode("nonblock") || mode("refused")))
        return NEXT(connect)(fd, addr, len);
    if (connecting_fd != -1) {
        TRACE("connect:twice");
        return NEXT(connect)(fd, addr, len);
    }
    /* A blocking EINTR leaves a real connection in flight. */
    int flags = fcntl(fd, F_GETFL);
    if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0)
        _exit(92);
    int result = NEXT(connect)(fd, addr, len);
    int err = errno;
    if (fcntl(fd, F_SETFL, flags) < 0 || (result < 0 && err != EINPROGRESS))
        _exit(93);
    connecting_fd = fd;
    INTERRUPT("connect:EINTR");
}

int poll(struct pollfd *fds, nfds_t n, int timeout) {
    if (connecting_fd >= 0 && n == 1 && fds[0].fd == connecting_fd && !polled++)
        INTERRUPT("poll:EINTR");
    return NEXT(poll)(fds, n, timeout);
}

int getsockopt(int fd, int level, int name, void *value, socklen_t *size) {
    if (fd == connecting_fd && level == SOL_SOCKET && name == SO_ERROR) {
        if (!checked_error++)
            INTERRUPT("SO_ERROR:EINTR");
        TRACE("connect:SO_ERROR");
    }
    return NEXT(getsockopt)(fd, level, name, value, size);
}

static int accepted;
int accept4(int fd, struct sockaddr *addr, socklen_t *size, int flags) {
    if (mode("io") && !accepted++)
        INTERRUPT("accept:EINTR");
    return NEXT(accept4)(fd, addr, size, flags);
}

int accept(int fd, struct sockaddr *addr, socklen_t *size) {
    if (mode("io") && !accepted++)
        INTERRUPT("accept:EINTR");
    return NEXT(accept)(fd, addr, size);
}

static int received;
ssize_t recv(int fd, void *data, size_t len, int flags) {
    if (mode("io") && !received++)
        INTERRUPT("recv:EINTR");
    return NEXT(recv)(fd, data, len, flags);
}

static int sends;
ssize_t send(int fd, const void *data, size_t len, int flags) {
    if (mode("io")) {
        ++sends;
        if (sends == 1)
            INTERRUPT("send:EINTR");
        if (sends == 4)
            INTERRUPT("sendall:EINTR-after-progress");
        if (len > 3)
            len = 3;
    }
    return NEXT(send)(fd, data, len, flags);
}
