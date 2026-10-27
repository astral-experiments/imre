#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

void *__real_malloc(size_t size);
uint64_t __real_imre_dyn_op(void *, const char *, uint64_t, uint64_t, uint64_t);
uint64_t __real_imre_dyn_op_at(void *, const char *, uint64_t, uint64_t, uint64_t, const uint32_t (*)[4]);

static _Thread_local int after_put, refused;
static _Thread_local unsigned remaining;

void *__wrap_malloc(size_t size) {
    if (remaining && size == 113 * sizeof(uint64_t) && --remaining == 0) {
        refused = 1;
        write(STDERR_FILENO, "refused\n", 8);
        errno = ENOMEM;
        return NULL;
    }
    return __real_malloc(size);
}

static int enter(const char *op) {
    if (!after_put || refused || strcmp(op, "call") != 0) return 0;
    after_put = 0;
    const char *ordinal = getenv("IMRE_TEST_GETTER_ALLOCATION");
    remaining = ordinal ? (unsigned)strtoul(ordinal, NULL, 10) : 0;
    return 1;
}

static uint64_t leave(const char *op, int probe, uint64_t result) {
    if (probe) remaining = 0;
    if (strcmp(op, "put") == 0) after_put = 1;
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
