#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

extern void *__real_malloc(size_t);
extern void *__real_realloc(void *, size_t);
extern uint32_t __real_imre_record_equal(const void *, const void *);
extern uint32_t __real_imre_sequence_equal(const void *, const void *);
extern uint32_t __real_imre_system_exit_code_equal(const void *, uint32_t, uintptr_t);

/* Fail once, inside the comparison only. Python error formatting stays usable. */
static int active, refused;
static int reject(void) {
    if (!active || refused) return 0;
    refused = 1;
    static const char message[] = "refused comparison allocation\n";
    (void)write(2, message, sizeof(message) - 1);
    errno = ENOMEM;
    return 1;
}
void *__wrap_malloc(size_t length) {
    return reject() ? NULL : __real_malloc(length);
}
void *__wrap_realloc(void *old, size_t length) {
    return reject() ? NULL : __real_realloc(old, length);
}
uint32_t __wrap_imre_record_equal(const void *a, const void *b) {
    int previous = active;
    active = getenv("IMRE_REFUSE_COMPARISON") != NULL;
    uint32_t answer = __real_imre_record_equal(a, b);
    active = previous;
    return answer;
}
uint32_t __wrap_imre_system_exit_code_equal(const void *a, uint32_t kind, uintptr_t b) {
    int previous = active;
    active = getenv("IMRE_REFUSE_COMPARISON") != NULL;
    uint32_t answer = __real_imre_system_exit_code_equal(a, kind, b);
    active = previous;
    return answer;
}
uint32_t __wrap_imre_sequence_equal(const void *a, const void *b) {
    int previous = active;
    active = getenv("IMRE_REFUSE_COMPARISON") != NULL;
    uint32_t answer = __real_imre_sequence_equal(a, b);
    active = previous;
    return answer;
}
