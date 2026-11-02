#include <stddef.h>
#include <stdint.h>
#include <wchar.h>

/* The caller holds the locale lock and provides one output slot per input byte.
 * Return the first undecodable byte offset, or length on success. */
size_t imre_decode_locale_name(const char *name, size_t length,
                               uint32_t *points, size_t *count, int surrogateescape) {
    mbstate_t state = {0};
    size_t offset = 0;
    *count = 0;
    while (offset < length) {
        wchar_t ch;
        size_t size = mbrtowc(&ch, name + offset, length - offset + 1, &state);
        if (size == 0) {
            return length;
        }
        if (size != (size_t)-1 && size != (size_t)-2) {
            uint32_t point = (uint32_t)ch;
            if (point <= 0x10ffff && !(point >= 0xd800 && point <= 0xdfff)) {
                points[(*count)++] = point;
                offset += size;
                continue;
            }
        }
        if (!surrogateescape) {
            break;
        }
        points[(*count)++] = 0xdc00 + (unsigned char)name[offset++];
        state = (mbstate_t){0};
    }
    return offset;
}
