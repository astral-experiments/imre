/* The native lookup requests each patched OS dependency as needed. */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>
extern const void *imre_bytes_literal(const unsigned char *, size_t);
extern const void *imre_str_literal(const unsigned char *, size_t);
extern const void *imre_exception_new(uint32_t, const void *);
extern void imre_error_set(const void *), imre_str_release(const void *), imre_exception_release(const void *);
extern const void *__real_imre_shutil_confstr_cs_path(void), *__real_imre_shutil_os_defpath(void);
extern size_t __real_confstr(int, char *, size_t);

static void receive(int fd, void *data, size_t length) {
    unsigned char *p = data;
    while (length) {
        ssize_t n = read(fd, p, length);
        if (n <= 0) _exit(73);
        p += n;
        length -= (size_t)n;
    }
}

static const void *request(char key, int fd) {
    unsigned char event[] = {(unsigned char)key, getenv("PATH") != NULL};
    if (write(fd, event, sizeof(event)) != sizeof(event)) _exit(72);
    uint32_t tag, length;
    receive(fd, &tag, sizeof(tag));
    receive(fd, &length, sizeof(length));
    size_t n = length == UINT32_MAX ? 0 : length;
    unsigned char *data = malloc(n + 1);
    if (!data) _exit(74);
    receive(fd, data, n);
    const void *value = NULL;
    if (tag == 1) {
        value = imre_bytes_literal(data, n);
    } else if (tag != 0) {
        const void *arg = length == UINT32_MAX ? NULL : imre_str_literal(data, n);
        const void *exc = imre_exception_new(tag, arg);
        imre_error_set(exc);
        imre_exception_release(exc);
        if (arg) imre_str_release(arg);
    }
    free(data);
    return value;
}

const void *__wrap_imre_shutil_confstr_cs_path(void) {
    const char *fd = getenv("IMRE_WHICH_FIXTURE_FD");
    return fd ? request('C', atoi(fd)) : __real_imre_shutil_confstr_cs_path();
}

const void *__wrap_imre_shutil_os_defpath(void) {
    const char *fd = getenv("IMRE_WHICH_FIXTURE_FD");
    return fd ? request('D', atoi(fd)) : __real_imre_shutil_os_defpath();
}

/* Keep the actual native POSIX provider for these two confstr fault controls. */
size_t __wrap_confstr(int name, char *buf, size_t size) {
    const char *fault = getenv("IMRE_WHICH_FIXTURE_CONFSTR");
    if (name == _CS_PATH && fault) {
        errno = fault[0] == 'E' ? EIO : 0;
        return 0;
    }
    return __real_confstr(name, buf, size);
}
