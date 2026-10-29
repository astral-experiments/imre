#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static _Thread_local unsigned armed, active, count, target, persistent, refused;
void *__real_malloc(size_t);
void *__real_realloc(void *, size_t);
void *__real_calloc(size_t, size_t);
uint64_t __real_imre_dyn_lit(void *, int, const unsigned char *, size_t);
uint64_t __real_imre_dyn_call_at(void *, uint64_t, uint64_t, uint64_t, size_t, const uint32_t (*)[4]);

static int deny(size_t size) {
    if (!active) return 0;
    unsigned decision = ++count == target || persistent;
    active = 0;
    dprintf(2, "noncallable_allocation %u %zu %u\n", count, size, decision);
    active = 1;
    if (decision) {
        refused = 1;
        errno = ENOMEM;
    }
    return decision;
}
void *__wrap_malloc(size_t size) { return deny(size) ? NULL : __real_malloc(size); }
void *__wrap_realloc(void *ptr, size_t size) { return deny(size) ? NULL : __real_realloc(ptr, size); }
void *__wrap_calloc(size_t count, size_t size) { if (count && size > SIZE_MAX / count) return __real_calloc(count, size); return deny(count * size) ? NULL : __real_calloc(count, size); }

uint64_t __wrap_imre_dyn_lit(void *ctx, int kind, const unsigned char *data, size_t len) {
    static const char marker[] = "arm noncallable diagnostic refusal";
    uint64_t result = __real_imre_dyn_lit(ctx, kind, data, len);
    if (kind == 1 && len == sizeof(marker) - 1 && !memcmp(data, marker, len)) armed = 1;
    return result;
}

uint64_t __wrap_imre_dyn_call_at(void *ctx, uint64_t callable, uint64_t args, uint64_t kwargs, size_t mark, const uint32_t (*span)[4]) {
    if (!armed) return __real_imre_dyn_call_at(ctx, callable, args, kwargs, mark, span);
    armed = 0;
    const char *setting = getenv("NONCALLABLE_REFUSE_NTH");
    target = setting ? strtoul(setting, NULL, 10) : 0;
    persistent = getenv("NONCALLABLE_REFUSE_ALL") != NULL;
    count = refused = 0;
    active = 1;
    uint64_t result = __real_imre_dyn_call_at(ctx, callable, args, kwargs, mark, span);
    active = 0;
    dprintf(2, "noncallable_summary %u %u\n", count, refused);
    return result;
}
