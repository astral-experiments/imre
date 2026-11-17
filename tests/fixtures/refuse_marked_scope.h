/* Apply a fixture's allocation probe to the next marked Python call. */
extern uint64_t __real_imre_dyn_lit(void *, int, const unsigned char *, size_t);
extern uint64_t __real_imre_dyn_linked_string(void *, const void *, int, const unsigned char *, size_t);
extern uint64_t __real_imre_dyn_scoped_call_at(void *, uint64_t, uint64_t, uint64_t, size_t, const uint32_t (*)[4], uint64_t);
extern uint64_t __real_imre_dyn_positional_call_at(void *, uint64_t, const uint64_t *, size_t, size_t, const uint32_t (*)[4], uint64_t);
extern uint64_t __real_imre_dyn_keyword_call_at(void *, uint64_t, const uint64_t *, size_t, const char *, size_t, size_t, const uint32_t (*)[4], uint64_t);

static void arm_marker(int kind, const unsigned char *data, size_t length) {
    static const char marker[] = REFUSAL_MARKER;
    if (kind == 1 && length == sizeof(marker) - 1 && !memcmp(data, marker, length)) armed = 1;
}
uint64_t __wrap_imre_dyn_lit(void *ctx, int kind, const unsigned char *data, size_t length) {
    uint64_t result = __real_imre_dyn_lit(ctx, kind, data, length);
    arm_marker(kind, data, length);
    return result;
}
uint64_t __wrap_imre_dyn_linked_string(void *ctx, const void *slot, int kind, const unsigned char *data, size_t length) {
    uint64_t result = __real_imre_dyn_linked_string(ctx, slot, kind, data, length);
    arm_marker(kind, data, length);
    return result;
}
static int begin_marked_call(void) {
    if (!armed) return 0;
    armed = 0;
    begin_probe();
    return 1;
}
static void end_marked_call(int tracking) { if (tracking) active = 0; }
uint64_t __wrap_imre_dyn_scoped_call_at(void *ctx, uint64_t callable, uint64_t args, uint64_t kwargs, size_t mark, const uint32_t (*span)[4], uint64_t scope) {
    int tracking = begin_marked_call();
    uint64_t result = __real_imre_dyn_scoped_call_at(ctx, callable, args, kwargs, mark, span, scope);
    end_marked_call(tracking);
    return result;
}
uint64_t __wrap_imre_dyn_positional_call_at(void *ctx, uint64_t callable, const uint64_t *values, size_t count, size_t mark, const uint32_t (*span)[4], uint64_t scope) {
    int tracking = begin_marked_call();
    uint64_t result = __real_imre_dyn_positional_call_at(ctx, callable, values, count, mark, span, scope);
    end_marked_call(tracking);
    return result;
}
uint64_t __wrap_imre_dyn_keyword_call_at(void *ctx, uint64_t callable, const uint64_t *values, size_t positional, const char *names, size_t keywords, size_t mark, const uint32_t (*span)[4], uint64_t scope) {
    int tracking = begin_marked_call();
    uint64_t result = __real_imre_dyn_keyword_call_at(ctx, callable, values, positional, names, keywords, mark, span, scope);
    end_marked_call(tracking);
    return result;
}
