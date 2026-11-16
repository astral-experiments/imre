#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

void *__real_realloc(void *ptr, size_t size);
uint64_t __real_imre_dyn_lookup(void *, uint64_t, const unsigned char *, size_t);

static _Thread_local int in_operand_lookup, refused;

void *__wrap_realloc(void *ptr, size_t size) {
    // Publishing this existing local grows the frame from eight to sixteen handles.
    if (in_operand_lookup && !refused && size == 16 * sizeof(uint64_t)) {
        refused = 1;
        write(STDERR_FILENO, "refused\n", 8);
        errno = ENOMEM;
        return NULL;
    }
    return __real_realloc(ptr, size);
}

uint64_t __wrap_imre_dyn_lookup(void *ctx, uint64_t scope, const unsigned char *name, size_t len) {
    static const char operand[] = "parameter_for_call_storage_with_a_deliberately_long_name";
    if (len != sizeof(operand) - 1 || memcmp(name, operand, len) != 0)
        return __real_imre_dyn_lookup(ctx, scope, name, len);

    const char *enabled = getenv("IMRE_TEST_GETTER_ALLOCATION");
    in_operand_lookup = enabled && strcmp(enabled, "1") == 0;
    uint64_t result = __real_imre_dyn_lookup(ctx, scope, name, len);
    in_operand_lookup = 0;
    return result;
}
