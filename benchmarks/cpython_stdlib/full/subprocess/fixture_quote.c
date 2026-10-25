/* Observe the actual runtime and optionally replace one returned command line. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

extern void *__real_imre_subprocess_quote_argument(uintptr_t, uint32_t);
extern void *__real_imre_str_join(const void *, const void *);
extern void imre_str_release(const void *);
extern void *imre_str_literal(const unsigned char *, size_t);

static unsigned int joined;
static unsigned int pending_quotes;

static void mark(int symbol) {
    const char *name = getenv("IMRE_QUOTE_EVENTS");
    if (!name)
        _exit(91);
    FILE *stream = fopen(name, "ab");
    if (!stream || fputc(symbol, stream) == EOF || fclose(stream))
        _exit(92);
}

void *__wrap_imre_subprocess_quote_argument(uintptr_t word, uint32_t kind) {
    void *result = __real_imre_subprocess_quote_argument(word, kind);
    if (result) {
        mark('Q');
        ++pending_quotes;
    }
    return result;
}

void *__wrap_imre_str_join(const void *separator, const void *values) {
    void *result = __real_imre_str_join(separator, values);
    /* unittest also joins text when formatting a failed comparison. Only the
       eight joins fed by subprocess's actual quoted words are under test. */
    if (result && pending_quotes) {
        pending_quotes = 0;
        mark('J');
        ++joined;
        const char *wrong = getenv("IMRE_QUOTE_CORRUPT");
        if (wrong && strtoul(wrong, NULL, 10) == joined) {
            static const unsigned char replacement[] = "control: wrong command line";
            imre_str_release(result);
            return imre_str_literal(replacement, sizeof(replacement) - 1);
        }
    }
    return result;
}
