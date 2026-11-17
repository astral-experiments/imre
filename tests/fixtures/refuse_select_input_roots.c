#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static unsigned armed, active, refused, nth, matches;
static size_t selected_size;
extern void *__real_malloc(size_t);
extern void *__real_calloc(size_t, size_t);
extern void *__real_realloc(void *, size_t);

static int reject(size_t size) {
    if (!active || refused || size != selected_size || ++matches != nth) return 0;
    refused = 1;
    static const char marker[] = "select_input_root_allocation_refused\n";
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
    const char *setting = getenv("SELECT_ROOT_REFUSE_NTH");
    nth = setting ? (unsigned)strtoul(setting, NULL, 10) : 0;
    setting = getenv("SELECT_ROOT_REFUSE_BYTES");
    selected_size = setting ? strtoul(setting, NULL, 10) : 8192;
    active = nth != 0;
}
#define REFUSAL_MARKER "arm select root refusal"
#include "refuse_marked_scope.h"
