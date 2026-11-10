#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

void *__real_malloc(size_t size);
const void *__real_imre_exception_at_frame(const void *, const void *, size_t, const void *,
                                         size_t, uint32_t, uint32_t, uint32_t, uint32_t);

static unsigned remaining;

void *__wrap_malloc(size_t size) {
    if (remaining && --remaining == 0) {
        write(STDERR_FILENO, "refused\n", 8);
        errno = ENOMEM;
        return NULL;
    }
    return __real_malloc(size);
}

const void *__wrap_imre_exception_at_frame(const void *value, const void *file, size_t file_len,
                                          const void *name, size_t name_len, uint32_t line,
                                          uint32_t start, uint32_t split, uint32_t end) {
    const char *ordinal = getenv("IMRE_TEST_RAISE_SITE_ALLOCATION");
    remaining = value && ordinal ? (unsigned)strtoul(ordinal, NULL, 10) : 0;
    const void *result = __real_imre_exception_at_frame(value, file, file_len, name, name_len,
                                                       line, start, split, end);
    remaining = 0;
    return result;
}
