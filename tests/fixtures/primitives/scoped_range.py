"""Tuple component moves retain their values on the last scalar iteration."""

def fold(start: int, stop: int, total: int) -> tuple[int, int, int, str]:
    label = "λé"
    ordinal = -1
    pair = (17, label)
    for ordinal in range(start, stop):
        pair = (2 * ordinal + 3, label)
        if ordinal % 7 == 2:
            total += 19
        total += pair[0] + len(pair[1])
    return total, ordinal, pair[0], pair[1]


def carried(stop: int) -> int:
    label = "old"
    pair = (9, label)
    total = 0
    for ordinal in range(stop):
        total += pair[0]
        pair = (ordinal, label)
    return total


for length in [0, 1, 1022, 1023, 2046, 2047]:
    print(fold(3, 3 + length, -5))
    print(carried(length))
print(fold(-2000, 3200, 2**75))
