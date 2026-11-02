"""A batched range must publish the same locals as the last scalar iteration."""


def add_rows(start: int, stop: int, total: int) -> tuple[int, int, int]:
    ordinal = 13
    term = 17
    for ordinal in range(start, stop):
        term = 3 * ordinal + 5
        total += term
        total += ordinal
    return total, ordinal, term


for length in [-1, 0, 1, 1023, 1024, 1025, 2049]:
    print(add_rows(7, 7 + length, -3))
print(add_rows(-1500, 1500, 11))
print(add_rows(0, 3000, (1 << 62) - 5))
print(add_rows(1 << 65, (1 << 65) + 2000, 3))
