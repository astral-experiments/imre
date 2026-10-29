#include <errno.h>
#include <poll.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

static unsigned active, refused, matches, nth, polls;
extern void *__real_malloc(size_t);
extern void *__real_calloc(size_t, size_t);
extern void *__real_realloc(void *, size_t);
extern int __real_poll(struct pollfd *, nfds_t, int);

static int reject(size_t size) {
    if (!active || refused || size != 2 * sizeof(uint64_t) || ++matches != nth) return 0;
    refused = 1;
    static const char marker[] = "poll_result_allocation_refused\n";
    (void)write(2, marker, sizeof(marker) - 1);
    errno = ENOMEM;
    return 1;
}
void *__wrap_malloc(size_t size) {
    return reject(size) ? NULL : __real_malloc(size);
}
void *__wrap_calloc(size_t count, size_t size) {
    size_t total = size && count > SIZE_MAX / size ? SIZE_MAX : count * size;
    return reject(total) ? NULL : __real_calloc(count, size);
}
void *__wrap_realloc(void *pointer, size_t size) {
    return reject(size) ? NULL : __real_realloc(pointer, size);
}
int __wrap_poll(struct pollfd *fds, nfds_t count, int timeout) {
    int result = __real_poll(fds, count, timeout);
    if (++polls == 1 && result > 0) {
        const char *setting = getenv("POLL_RESULT_REFUSE_NTH");
        nth = setting ? (unsigned)strtoul(setting, NULL, 10) : 0;
        active = nth != 0;
    }
    return result;
}
