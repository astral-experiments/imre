#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static unsigned armed, active, refused;
extern void *__real_malloc(size_t);
extern void *__real_calloc(size_t, size_t);
extern void *__real_realloc(void *, size_t);

static int reject(size_t size) {
    if (!active || refused || size != 3 * sizeof(uint64_t)) return 0;
    refused = 1;
    static const char marker[] = "select_result_allocation_refused\n";
    (void)write(2, marker, sizeof(marker) - 1);
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
static void begin_probe(void) {
    const char *setting = getenv("SELECT_RESULT_REFUSE");
    active = setting && setting[0] == '1';
}
#define REFUSAL_MARKER "arm select result refusal"
#include "refuse_marked_scope.h"
