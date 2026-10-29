#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void *__real_malloc(size_t);
void *__real_realloc(void *, size_t);
void *__real_calloc(size_t, size_t);
uint64_t __real_imre_dyn_lit(void *, int, const unsigned char *, size_t);
uint64_t __real_imre_dyn_op(void *, const char *, uint64_t, uint64_t, uint64_t);
uint64_t __real_imre_dyn_op_at(void *, const char *, uint64_t, uint64_t, uint64_t, const uint32_t (*)[4]);
static _Thread_local unsigned armed, active, calls, allocations, nth;
static int deny(size_t size) {
    if (!active || size < 256) return 0;
    unsigned decision = ++allocations == nth;
    active = 0;
    dprintf(2, "weak_proxy_root_allocation %u %u %zu %u\n", calls, allocations, size, decision);
    if (decision) {
        armed = 0;
        errno = ENOMEM;
    }
    active = armed;
    return decision;
}
void *__wrap_malloc(size_t size) { return deny(size) ? NULL : __real_malloc(size); }
void *__wrap_realloc(void *ptr, size_t size) { return deny(size) ? NULL : __real_realloc(ptr, size); }
void *__wrap_calloc(size_t count, size_t size) { if (count && size > SIZE_MAX / count) return __real_calloc(count, size); return deny(count * size) ? NULL : __real_calloc(count, size); }
uint64_t __wrap_imre_dyn_lit(void *ctx, int kind, const unsigned char *data, size_t len) {
    static const char marker[] = "arm weak proxy root refusal";
    uint64_t result = __real_imre_dyn_lit(ctx, kind, data, len);
    if (kind == 1 && len == sizeof(marker) - 1 && !memcmp(data, marker, len)) {
        armed = 1;
        const char *value = getenv("PROXY_ROOT_REFUSE_NTH");
        nth = value ? (unsigned)strtoul(value, NULL, 10) : 0;
    }
    return result;
}
static int enter(const char *op) {
    if (armed && !strcmp(op, "get")) { ++calls; active = 1; return 1; }
    return 0;
}
uint64_t __wrap_imre_dyn_op(void *ctx, const char *op, uint64_t a, uint64_t b, uint64_t c) {
    int probe = enter(op);
    uint64_t result = __real_imre_dyn_op(ctx, op, a, b, c);
    if (probe) active = 0;
    return result;
}
uint64_t __wrap_imre_dyn_op_at(void *ctx, const char *op, uint64_t a, uint64_t b, uint64_t c, const uint32_t (*span)[4]) {
    int probe = enter(op);
    uint64_t result = __real_imre_dyn_op_at(ctx, op, a, b, c, span);
    if (probe) active = 0;
    return result;
}
__attribute__((destructor)) static void stats(void) { dprintf(2, "weak_proxy_root_stats %u %u\n", calls, allocations); }
