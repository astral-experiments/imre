/* Linux linker probe. Refuse one allocation in each manager constructor. */
#include <assert.h>
#include <stddef.h>
#include <stdint.h>

extern void *__real_malloc(size_t size);
extern void *__real_imre_log_manager(void *logger);
extern void *__real_imre_log_manager_new(uintptr_t root);
extern uint32_t imre_error_pending(void);

static int refuse_next;
static int shared_attempts;
static int private_attempts;

void *__wrap_malloc(size_t size) {
    if (refuse_next) {
        refuse_next = 0;
        return NULL;
    }
    return __real_malloc(size);
}

void *__wrap_imre_log_manager(void *logger) {
    int fail = shared_attempts++ == 0;
    refuse_next = fail;
    void *result = __real_imre_log_manager(logger);
    if (fail) {
        assert(!refuse_next && !result && imre_error_pending());
    }
    return result;
}

void *__wrap_imre_log_manager_new(uintptr_t root) {
    int fail = private_attempts++ == 0;
    refuse_next = fail;
    void *result = __real_imre_log_manager_new(root);
    if (fail) {
        assert(!refuse_next && !result && imre_error_pending());
    }
    return result;
}
