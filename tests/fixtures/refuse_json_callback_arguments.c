#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static unsigned armed, active, refused, seen;
extern void *__real_malloc(size_t);
extern void *__real_calloc(size_t, size_t);
extern void *__real_realloc(void *, size_t);
extern uint64_t __real_imre_dyn_lit(void *, int, const unsigned char *, size_t);
extern uint64_t __real_imre_dyn_call_at(void *, uint64_t, uint64_t, uint64_t, size_t, const uint32_t (*)[4]);

static int reject(size_t size) {
    if (!active || refused || size != 8) return 0;
    const char *setting = getenv("IMRE_TEST_JSON_CALLBACK_NTH");
    unsigned nth = setting ? (unsigned)strtoul(setting, NULL, 10) : 0;
    if (++seen != nth) return 0;
    refused = 1;
    static const char message[] = "refused\n";
    ssize_t written = write(2, message, sizeof(message) - 1);
    (void)written;
    errno = ENOMEM;
    return 1;
}
void *__wrap_malloc(size_t size) { return reject(size) ? NULL : __real_malloc(size); }
void *__wrap_realloc(void *pointer, size_t size) { return reject(size) ? NULL : __real_realloc(pointer, size); }
void *__wrap_calloc(size_t count, size_t size) {
    size_t total = size && count > SIZE_MAX / size ? SIZE_MAX : count * size;
    return reject(total) ? NULL : __real_calloc(count, size);
}
uint64_t __wrap_imre_dyn_lit(void *ctx, int kind, const unsigned char *data, size_t length) {
    uint64_t result = __real_imre_dyn_lit(ctx, kind, data, length);
    static const char marker[] = "arm JSON default argument refusal";
    if (kind == 1 && length == sizeof(marker) - 1 && !memcmp(data, marker, length)) armed = 1;
    return result;
}
uint64_t __wrap_imre_dyn_call_at(void *ctx, uint64_t callable, uint64_t args,
                               uint64_t kwargs, size_t mark, const uint32_t (*span)[4]) {
    if (!armed) return __real_imre_dyn_call_at(ctx, callable, args, kwargs, mark, span);
    armed = 0;
    active = 1;
    uint64_t result = __real_imre_dyn_call_at(ctx, callable, args, kwargs, mark, span);
    active = 0;
    return result;
}
