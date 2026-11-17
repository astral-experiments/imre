#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static unsigned armed, active, refused, seen;
extern void *__real_malloc(size_t);
extern void *__real_calloc(size_t, size_t);
extern void *__real_realloc(void *, size_t);

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
static void begin_probe(void) {
    active = 1;
}
#define REFUSAL_MARKER "arm JSON default argument refusal"
#include "refuse_marked_scope.h"
