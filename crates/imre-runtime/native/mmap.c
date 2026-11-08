#ifdef __linux__
#define _GNU_SOURCE
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

// A shared file mapping can change independently of a Rust borrow. Copy only
// the requested bytes across the native boundary; never interpret mapped
// storage as a Rust value. A range transfer need not be atomic as a whole.
_Static_assert(__atomic_always_lock_free(1, 0), "Linux mmap requires lock-free byte accesses");

void imre_mmap_read(unsigned char *dst, const volatile unsigned char *map, size_t len) {
    for (size_t i = 0; i < len; i++) {
        dst[i] = __atomic_load_n(map + i, __ATOMIC_RELAXED);
    }
}

void imre_mmap_write(volatile unsigned char *map, const unsigned char *src, size_t len) {
    for (size_t i = 0; i < len; i++) {
        __atomic_store_n(map + i, src[i], __ATOMIC_RELAXED);
    }
}

void imre_mmap_copy(unsigned char *dst, int64_t step, const unsigned char *src, size_t len) {
    if (step == 1) {
        // Contiguous ranges can overlap within the same mapping.
        memmove(dst, src, len);
        return;
    }
    for (size_t i = 0; i < len; i++) {
        unsigned char byte = __atomic_load_n(src + i, __ATOMIC_RELAXED);
        __atomic_store_n(dst, byte, __ATOMIC_RELAXED);
        if (i + 1 < len) {
            dst += step;
        }
    }
}

ptrdiff_t imre_mmap_find(const unsigned char *data, size_t len, const unsigned char *needle,
                         size_t needle_len, bool reverse) {
    if (needle_len > len) {
        return -1;
    }
    if (needle_len == 0) {
        return reverse ? (ptrdiff_t)len : 0;
    }
    if (!reverse) {
        const unsigned char *hit = memmem(data, len, needle, needle_len);
        return hit ? hit - data : -1;
    }

    // Test the last possible first-byte hit before searching any earlier starts.
    const unsigned char *tail = memrchr(data, needle[0], len - needle_len + 1);
    if (!tail || needle_len == 1 || memcmp(tail, needle, needle_len) == 0) {
        return tail ? tail - data : -1;
    }

    // Search the upper half of the possible starts with libc's bounded
    // substring search. This avoids a full comparison at every shared prefix.
    size_t lo = 0, hi = (size_t)(tail - data);
    const unsigned char *last = NULL;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        const unsigned char *hit = memmem(data + mid, hi - mid + (needle_len - 1),
                                          needle, needle_len);
        if (hit) {
            last = hit;
            lo = (size_t)(hit - data) + 1;
        } else {
            hi = mid;
        }
    }
    return last ? last - data : -1;
}
#endif
