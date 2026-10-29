#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

void *__real_malloc(size_t);
void *__real_realloc(void *, size_t);
uint64_t __real_imre_dyn_lit(void *, int, const unsigned char *, size_t);
static _Thread_local unsigned remaining;
static _Thread_local int ready, refused;

static int refuse(void) {
    if (remaining && --remaining == 0) {
        refused = 1;
        write(2, "refused\n", 8);
        errno = ENOMEM;
        return 1;
    }
    return 0;
}

void *__wrap_malloc(size_t n) { return refuse() ? NULL : __real_malloc(n); }
void *__wrap_realloc(void *p, size_t n) { return refuse() ? NULL : __real_realloc(p, n); }

uint64_t __wrap_imre_dyn_lit(void *ctx, int kind, const unsigned char *data, size_t len) {
    static const char marker[] = "start formatter allocation probe 2332";
    static const char name[] = "parameter_for_call_storage_with_a_deliberately_long_name";
    int probe = ready && !refused && kind == 6 && len == sizeof(name)-1 && !memcmp(data,name,len);
    if (probe) {
        const char *ordinal = getenv("IMRE_TEST_GETTER_ALLOCATION");
        remaining = ordinal ? (unsigned)strtoul(ordinal,NULL,10) : 0;
    }
    uint64_t result = __real_imre_dyn_lit(ctx,kind,data,len);
    if (probe) remaining = 0;
    if (kind == 1 && len == sizeof(marker)-1 && !memcmp(data,marker,len)) ready = 1;
    return result;
}
