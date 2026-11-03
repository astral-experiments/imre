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


def discarded_product(start: int, stop: int) -> tuple[int, int]:
    product = 0
    total = 0
    for i in range(start, stop):
        product = i * 2_147_483_647
        total += product * 0 + 1
    return total, product


for size in [0, 1, 1022, 1023, 1024, 2046]:
    print(fold(3, 3 + size, -7))
print(fold(-2000, 3000, 1 << 62))
print(fold(0, 4096, (1 << 62) - 10))
# A later block crosses the tagged-sum limit or the ordinal bound.
print(fold(0, 4096, (1 << 62) - 13_000_000))
print(fold((1 << 31) - 3000, (1 << 31) + 5, 7))

print(fold(1 << 33, (1 << 33) + 4097, -11))
print(discarded_product(1 << 32, (1 << 32) + 2048))

# Keep the original owner live while the reduction replaces its boxed accumulator.
for initial in [1 << 100, -(1 << 100), -(1 << 62) - 2000]:
    print(fold(0, 4097, initial))
    print(initial)
