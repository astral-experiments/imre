#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

size_t confstr(int name, char *buffer, size_t size) {
    static unsigned queries;
    if (name != _CS_GNU_LIBPTHREAD_VERSION) {
        size_t (*real_confstr)(int, char *, size_t) = dlsym(RTLD_NEXT, "confstr");
        return real_confstr(name, buffer, size);
    }
    if (++queries != 1) abort();
    const char *value = getenv("IMRE_TEST_PTHREAD_VERSION");
    if (!value || !*value) return 0;
    size_t length = strlen(value) + 1;
    if (size) {
        size_t n = length < size ? length : size;
        memcpy(buffer, value, n);
        buffer[n - 1] = 0;
    }
    return length;
}
