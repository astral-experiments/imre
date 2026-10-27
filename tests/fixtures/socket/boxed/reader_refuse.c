#define _GNU_SOURCE
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

extern int __real_accept4(int, struct sockaddr *, socklen_t *, int);
extern void *__real_malloc(size_t), *__real_calloc(size_t, size_t), *__real_realloc(void *, size_t);
static const char *target;
static int buffers, next_owner;

int __wrap_accept4(int fd, struct sockaddr *addr, socklen_t *length, int flags) {
    int out = __real_accept4(fd, addr, length, flags);
    if (out >= 0)
        target = getenv("IMRE_SOCKET_READER_FAIL");
    return out;
}

/* The second reader is built while the first keeps the closed socket alive. */
static int refuse(size_t size) {
    if (!target)
        return 0;
    if (size == 128 * 1024) {
        if (++buffers != 2)
            return 0;
        if (strcmp(target, "owner") == 0) {
            next_owner = 1;
            return 0;
        }
    } else if (!next_owner) {
        return 0;
    }
    target = NULL;
    const char line[] = "refused socket reader allocation\n";
    (void)write(2, line, sizeof(line) - 1);
    errno = ENOMEM;
    return 1;
}

void *__wrap_malloc(size_t size) { return refuse(size) ? NULL : __real_malloc(size); }
void *__wrap_calloc(size_t count, size_t size) {
    return refuse(count * size) ? NULL : __real_calloc(count, size);
}
void *__wrap_realloc(void *ptr, size_t size) {
    return refuse(size) ? NULL : __real_realloc(ptr, size);
}
