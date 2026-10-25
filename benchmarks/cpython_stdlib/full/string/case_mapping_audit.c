#include <stdint.h>
#include <stdio.h>
#include <stddef.h>
extern const void *imre_str_literal(const uint8_t *, size_t);
extern const void *imre_str_capitalize(const void *);
extern const void *imre_str_lower(const void *);
extern uint32_t imre_str_equal(const void *, const void *);
extern void imre_str_release(const void *);
int main(void) {
    size_t count = 0; int len;
    while ((len = fgetc(stdin)) != EOF) {
        int cap_len = fgetc(stdin), low_len = fgetc(stdin);
        uint8_t buf[32], cap[32], low[32];
        if (len > 32 || cap_len < 0 || cap_len > 32 || low_len < 0 || low_len > 32 ||
          fread(buf, 1, len, stdin) != (size_t)len || fread(cap, 1, cap_len, stdin) != (size_t)cap_len ||
          fread(low, 1, low_len, stdin) != (size_t)low_len) return 2;
        const void *text = imre_str_literal(buf, len);
        const void *actual_c = imre_str_capitalize(text), *actual_l = imre_str_lower(text);
        const void *expected_c = imre_str_literal(cap, cap_len), *expected_l = imre_str_literal(low, low_len);
        if (!imre_str_equal(actual_c, expected_c) || !imre_str_equal(actual_l, expected_l)) {
            fprintf(stderr, "case %zu differs\n", count); return 1;
        }
        imre_str_release(text); imre_str_release(actual_c); imre_str_release(actual_l);
        imre_str_release(expected_c); imre_str_release(expected_l); ++count;
    }
    printf("%zu lowercase and capitalize pairs matched\n", count); return ferror(stdin);
}
