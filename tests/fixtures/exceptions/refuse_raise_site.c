#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

void *__real_malloc(size_t size);
const void *__real_imre_exception_with_origin(const void *, const void *, uint64_t);

static unsigned remaining;

void *__wrap_malloc(size_t size) {
    if (remaining && --remaining == 0) {
        write(STDERR_FILENO, "refused\n", 8);
        errno = ENOMEM;
        return NULL;
    }
    return __real_malloc(size);
}

const void *__wrap_imre_exception_with_origin(const void *value, const void *file, uint64_t line) {
    const char *ordinal = getenv("IMRE_TEST_RAISE_SITE_ALLOCATION");
    remaining = ordinal ? (unsigned)strtoul(ordinal, NULL, 10) : 0;
    const void *result = __real_imre_exception_with_origin(value, file, line);
    remaining = 0;
    return result;
}
