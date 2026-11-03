"""Periodic range sums preserve the last predicate, the ordinal and integer promotion."""


def fold(start: int, stop: int, total: int) -> tuple[int, int, bool]:
    label = "éλ"
    ordinal = -1
    selected = False
    for ordinal in range(start, stop):
        selected = ordinal % 7 == 2
        total += 3 * ordinal + len(label)
        if selected:
            total += 19
        else:
            total += 4
    return total, ordinal, selected


for size in [0, 1, 1022, 1023, 1024, 2046]:
    print(fold(3, 3 + size, -7))
print(fold(-2000, 3000, 1 << 62))
print(fold(0, 4096, (1 << 62) - 10))
# A later block crosses the tagged-sum limit or the ordinal bound.
print(fold(0, 4096, (1 << 62) - 13_000_000))
print(fold((1 << 31) - 3000, (1 << 31) + 5, 7))
