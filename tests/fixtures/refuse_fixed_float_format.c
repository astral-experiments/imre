#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static unsigned active, calls, refused, policy, target_call;
extern void *__real_malloc(size_t);
extern void *__real_calloc(size_t, size_t);
extern void *__real_realloc(void *, size_t);
extern const void *__real_imre_float_format(double, uint32_t, uint32_t);

static int reject(size_t size) {
    if (!active) return 0;
    if (policy == 1) {
        if (size < 64 || refused >= 2) return 0;
    } else if (policy == 2) {
        /* Shared<CompactString> has three string words and two count words. */
        if (size != 5 * sizeof(void *) || refused) return 0;
    } else {
        return 0;
    }
    if (!refused) {
        static const char marker[] = "fixed_float_allocation_refused\n";
        (void)write(2, marker, sizeof(marker) - 1);
    }
    ++refused;
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

const void *__wrap_imre_float_format(double value, uint32_t precision, uint32_t flags) {
    static unsigned configured;
    if (!configured) {
        const char *setting = getenv("FIXED_FLOAT_REFUSE");
        policy = setting && !strcmp(setting, "buffer") ? 1 :
                 setting && !strcmp(setting, "owner") ? 2 : 0;
        const char *ordinal = getenv("FIXED_FLOAT_CALL");
        target_call = ordinal ? (unsigned)strtoul(ordinal, NULL, 10) : 1;
        configured = 1;
    }
    active = ++calls == target_call;
    const void *result = __real_imre_float_format(value, precision, flags);
    active = 0;
    return result;
}
