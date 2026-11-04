#include <locale.h>
#include <pthread.h>
#include <stddef.h>
#include <stdio.h>
#include <stdint.h>
#include <wchar.h>

/* Python's percent formatting always uses an ASCII radix. This immutable C
 * locale is shared by threads, each of which restores its own previous locale. */
static locale_t numeric_c_locale;
static pthread_once_t numeric_c_once = PTHREAD_ONCE_INIT;

static void init_numeric_c_locale(void) {
    numeric_c_locale = newlocale(LC_NUMERIC_MASK, "C", (locale_t)0);
}

int imre_format_ascii_double(char *buffer, size_t size, const char *format, double value) {
    if (pthread_once(&numeric_c_once, init_numeric_c_locale) != 0 || !numeric_c_locale)
        return -1;
    locale_t previous = uselocale(numeric_c_locale);
    if (!previous)
        return -1;
    int written = snprintf(buffer, size, format, value);
    uselocale(previous);
    return written;
}

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

#if defined(__linux__) && defined(__GLIBC__)
#include <langinfo.h>
#include <locale.h>

/* nl_langinfo also accepts non-string GNU data, so only expose the Python
 * string-valued items and keep their decoding category alongside the value. */
struct imre_langinfo_item {
    const char *name;
    int value;
    int category;
};

#define ITEM(name, category) {#name, name, category}
static const struct imre_langinfo_item langinfo_items[] = {
    ITEM(DAY_1, LC_TIME), ITEM(DAY_2, LC_TIME), ITEM(DAY_3, LC_TIME),
    ITEM(DAY_4, LC_TIME), ITEM(DAY_5, LC_TIME), ITEM(DAY_6, LC_TIME),
    ITEM(DAY_7, LC_TIME), ITEM(ABDAY_1, LC_TIME), ITEM(ABDAY_2, LC_TIME),
    ITEM(ABDAY_3, LC_TIME), ITEM(ABDAY_4, LC_TIME), ITEM(ABDAY_5, LC_TIME),
    ITEM(ABDAY_6, LC_TIME), ITEM(ABDAY_7, LC_TIME), ITEM(MON_1, LC_TIME),
    ITEM(MON_2, LC_TIME), ITEM(MON_3, LC_TIME), ITEM(MON_4, LC_TIME),
    ITEM(MON_5, LC_TIME), ITEM(MON_6, LC_TIME), ITEM(MON_7, LC_TIME),
    ITEM(MON_8, LC_TIME), ITEM(MON_9, LC_TIME), ITEM(MON_10, LC_TIME),
    ITEM(MON_11, LC_TIME), ITEM(MON_12, LC_TIME), ITEM(ABMON_1, LC_TIME),
    ITEM(ABMON_2, LC_TIME), ITEM(ABMON_3, LC_TIME), ITEM(ABMON_4, LC_TIME),
    ITEM(ABMON_5, LC_TIME), ITEM(ABMON_6, LC_TIME), ITEM(ABMON_7, LC_TIME),
    ITEM(ABMON_8, LC_TIME), ITEM(ABMON_9, LC_TIME), ITEM(ABMON_10, LC_TIME),
    ITEM(ABMON_11, LC_TIME), ITEM(ABMON_12, LC_TIME), ITEM(D_T_FMT, LC_TIME),
    ITEM(D_FMT, LC_TIME), ITEM(T_FMT, LC_TIME), ITEM(AM_STR, LC_TIME),
    ITEM(PM_STR, LC_TIME), ITEM(T_FMT_AMPM, LC_TIME), ITEM(ERA, LC_TIME),
    ITEM(ERA_D_FMT, LC_TIME), ITEM(ERA_D_T_FMT, LC_TIME), ITEM(ERA_T_FMT, LC_TIME),
    ITEM(ALT_DIGITS, LC_TIME), ITEM(_DATE_FMT, LC_TIME),
    ITEM(CODESET, LC_CTYPE),
    ITEM(RADIXCHAR, LC_NUMERIC), ITEM(THOUSEP, LC_NUMERIC),
    ITEM(CRNCYSTR, LC_MONETARY),
    ITEM(YESEXPR, LC_MESSAGES), ITEM(NOEXPR, LC_MESSAGES),
};
#undef ITEM

const struct imre_langinfo_item *imre_langinfo_items(size_t *count)
{
    *count = sizeof(langinfo_items) / sizeof(langinfo_items[0]);
    return langinfo_items;
}
#endif
