/* Test transport for the original Python handlers. The native walk chooses
 * the handler and failing operation, supplies its exception, and waits until
 * the handler returns before making another filesystem call. */
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

typedef void (*handler)(const void *, uint32_t, const void *, const void *);
extern void __real_imre_shutil_rmtree(const void *, uint32_t, uint32_t, uintptr_t, uint32_t,
                                      handler, const void *, handler, const void *);
extern const void *imre_str_encode(const void *), *imre_exception_repr(const void *),
                  *imre_exception_message(const void *), *imre_int_str(uintptr_t);
extern uintptr_t imre_bytes_len(const void *), imre_bytes_get(const void *, uintptr_t),
                 imre_os_error_field(const void *, uint32_t);
extern void imre_bytes_release(const void *), imre_str_release(const void *),
            imre_int_release(uintptr_t), imre_error_set(const void *);

struct context { int fd; uint32_t legacy; };

static void send_all(int fd, const void *data, size_t count) {
    const unsigned char *at = data;
    while (count) {
        ssize_t n = write(fd, at, count);
        if (n <= 0) _exit(72);
        at += n;
        count -= (size_t)n;
    }
}

static void word(int fd, uint32_t n) { send_all(fd, &n, sizeof(n)); }

static void bytes(int fd, const void *value) {
    uint32_t length = (uint32_t)(imre_bytes_len(value) >> 1);
    word(fd, length);
    unsigned char buffer[256];
    for (uint32_t i = 0; i < length; ++i) {
        buffer[i % sizeof(buffer)] = (unsigned char)(imre_bytes_get(value, ((uintptr_t)i << 1) | 1) >> 1);
        if (i % sizeof(buffer) == sizeof(buffer) - 1 || i + 1 == length)
            send_all(fd, buffer, i % sizeof(buffer) + 1);
    }
}

static void text(int fd, const void *value) {
    const void *encoded = imre_str_encode(value);
    bytes(fd, encoded);
    imre_bytes_release(encoded);
}

static void owned_text(int fd, const void *value) {
    text(fd, value);
    imre_str_release(value);
}

static void callback(const void *raw, uint32_t op, const void *path, const void *error) {
    const struct context *ctx = raw;
    int fd = ctx->fd;
    word(fd, op);
    word(fd, ctx->legacy);
    text(fd, path);
    owned_text(fd, imre_exception_repr(error));
    owned_text(fd, imre_exception_message(error));
    for (uint32_t field = 0; field < 4; ++field) {
        uintptr_t v = imre_os_error_field(error, field);
        if (!v) { word(fd, 0); continue; }
        if (field == 0) {
            word(fd, 1);
            owned_text(fd, imre_int_str(v));
            imre_int_release(v);
        } else if (field > 1 && (v & 7) == 1) {
            const void *b = (const void *)(v & ~(uintptr_t)7);
            word(fd, 2); bytes(fd, b); imre_bytes_release(b);
        } else {
            word(fd, 1); owned_text(fd, (const void *)v);
        }
    }
    unsigned char reply;
    if (read(fd, &reply, 1) != 1) _exit(73);
    if (reply == 'R') imre_error_set(error);
    else if (reply != 'C') _exit(74);
}

void __wrap_imre_shutil_rmtree(const void *path, uint32_t tag, uint32_t ignore,
                               uintptr_t dir, uint32_t kind,
                               handler old, const void *octx, handler exc, const void *ectx) {
    const char *fd = getenv("IMRE_SHUTIL_FIXTURE_CALLBACK_FD");
    struct context onerror = {fd ? atoi(fd) : -1, 1}, onexc = {onerror.fd, 0};
    if (fd && getenv("IMRE_SHUTIL_FIXTURE_ONERROR")) { old = callback; octx = &onerror; }
    if (fd && getenv("IMRE_SHUTIL_FIXTURE_ONEXC")) { exc = callback; ectx = &onexc; }
    __real_imre_shutil_rmtree(path, tag, ignore, dir, kind, old, octx, exc, ectx);
}
