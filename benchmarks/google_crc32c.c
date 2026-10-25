#include <crc32c/crc32c.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Keep setup and I/O outside the same sustained loop used by driver.py. */
int main(int argc, char **argv) {
    if (argc != 4 || strcmp(argv[1], "measure") != 0) return 1;
    const int extend = strcmp(argv[2], "extend") == 0;
    if (!extend && strcmp(argv[2], "value") != 0) return 1;
    const size_t size = strtoull(argv[3], NULL, 10);
    if (size == 0 || size % 8 != 0) return 1;
    uint8_t *payload = malloc(size);
    if (!payload) return 1;
    for (size_t offset = 0; offset < size; offset += 8) {
        memcpy(payload + offset, "12345678", 8);
    }
    puts("crc32c-ready");
    fflush(stdout);
    unsigned long repetitions;
    while (scanf("%lu", &repetitions) == 1 && repetitions) {
        uint32_t crc = 1;
        if (extend) {
            for (unsigned long i = 0; i < repetitions; ++i) {
                crc = crc32c_extend(crc, payload, size);
            }
        } else {
            for (unsigned long i = 0; i < repetitions; ++i) {
                crc = crc32c_value(payload, size);
            }
        }
        printf("%" PRIu32 "\n", crc);
        fflush(stdout);
    }
    free(payload);
    return 0;
}
