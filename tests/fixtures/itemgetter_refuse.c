#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

void *__real_malloc(size_t size);
void *__real_realloc(void *ptr, size_t size);
uint64_t __real_imre_dyn_lit(void *, int, const unsigned char *, size_t);
uint64_t __real_imre_dyn_op(void *, const char *, uint64_t, uint64_t, uint64_t);
uint64_t __real_imre_dyn_op_at(void *, const char *, uint64_t, uint64_t, uint64_t, const uint32_t (*)[4]);
uint64_t __real_imre_dyn_call_at(void *, uint64_t, uint64_t, uint64_t, size_t, const uint32_t (*)[4]);

static _Thread_local int after_put, refused, in_call;
static _Thread_local unsigned remaining;
static _Thread_local size_t allocation_size;

static int refuse(size_t size) {
    if ((remaining && size == allocation_size && --remaining == 0) ||
        (in_call && refused && size == 11 && getenv("IMRE_TEST_MEMORY_ERROR_KIND"))) {
        refused = 1;
        write(STDERR_FILENO, "refused\n", 8);
        errno = ENOMEM;
        return 1;
    }
    return 0;
}

void *__wrap_malloc(size_t size) {
    return refuse(size) ? NULL : __real_malloc(size);
}

void *__wrap_realloc(void *ptr, size_t size) {
    return refuse(size) ? NULL : __real_realloc(ptr, size);
}

static int enter(const char *op) {
    const char *wanted = getenv("IMRE_TEST_CALL_OP");
    if (!after_put || refused || strcmp(op, wanted ? wanted : "call") != 0) return 0;
    after_put = 0;
    in_call = 1;
    const char *size = getenv("IMRE_TEST_CALL_SIZE");
    allocation_size = size ? (size_t)strtoul(size, NULL, 10) : 113 * sizeof(uint64_t);
    const char *ordinal = getenv("IMRE_TEST_GETTER_ALLOCATION");
    remaining = ordinal ? (unsigned)strtoul(ordinal, NULL, 10) : 0;
    return 1;
}

static uint64_t leave(const char *op, int probe, uint64_t result) {
    if (probe) { remaining = 0; in_call = 0; }
    if (strcmp(op, "put") == 0 && !getenv("IMRE_TEST_CALL_SIZE")) after_put = 1;
    return result;
}

uint64_t __wrap_imre_dyn_op_at(void *ctx, const char *op, uint64_t a, uint64_t b, uint64_t c, const uint32_t (*span)[4]) {
    int probe = enter(op);
    return leave(op, probe, __real_imre_dyn_op_at(ctx, op, a, b, c, span));
}

uint64_t __wrap_imre_dyn_op(void *ctx, const char *op, uint64_t a, uint64_t b, uint64_t c) {
    int probe = enter(op);
    return leave(op, probe, __real_imre_dyn_op(ctx, op, a, b, c));
}

uint64_t __wrap_imre_dyn_call_at(void *ctx, uint64_t fn, uint64_t args, uint64_t kwargs, size_t mark, const uint32_t (*span)[4]) {
    int probe = enter("call");
    return leave("call", probe, __real_imre_dyn_call_at(ctx, fn, args, kwargs, mark, span));
}

uint64_t __wrap_imre_dyn_lit(void *ctx, int kind, const unsigned char *data, size_t len) {
    static const char marker[] = "start formatter allocation probe 2332";
    uint64_t result = __real_imre_dyn_lit(ctx, kind, data, len);
    if (kind == 1 && len == sizeof(marker) - 1 && !memcmp(data, marker, len)) after_put = 1;
    return result;
}
