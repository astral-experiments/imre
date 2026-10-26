"""Unknown integer parameters exercise checked tagged arithmetic and fallback."""

import sys


def arithmetic(left: int, right: int) -> None:
    print(left + right)
    print(left - right)
    print(left * right)
    # Promoted intermediates can demote before another arithmetic operation.
    print((left + right) - right)
    print((left - right) + right)
    print((left * right) * 0 + 1)


def transitions(value: int, step: int) -> None:
    original = value
    for repeat in range(8):
        value += step
        print(value)
        value -= step
        print(value)
    print(original)
    print(value)


def unwound_integer(value: int) -> int:
    kept = value + 1
    try:
        # The result slot remains unset when strict zip raises. Heap-valued
        # private targets and the partial sum still need their owners released.
        unused = sum(
            item
            for item, marker in zip([value + 2], [0, 1], strict=True)
        )
    except ValueError:
        value, kept = kept, value
    else:
        return unused
    return value + kept


class Counter:
    total: int

    def __init__(self, total: int) -> None:
        self.total = total


def count(value: int, flag: bool) -> None:
    counter = Counter(value)
    items = [value]
    mapping = {"total": value}
    counter.total += flag
    items[0] += flag
    mapping["total"] += flag
    print(value + flag)
    print(flag + value)
    print(flag + flag)
    value += flag
    assert value == counter.total == items[0] == mapping["total"]
    print(value)


def plus(value: int) -> int:
    return value + 1

def compare(value: int) -> bool:
    return plus(value) > 0


boundary = 1 << 62
huge = 1 << 150
# The existing input-driven arithmetic matrix omits right-hand zero so it can
# also test division. Cover that missing operand for small and heap integers.
for value in [
    -huge,
    -boundary - 1,
    -boundary,
    -boundary + 1,
    -1,
    0,
    1,
    boundary - 2,
    boundary - 1,
    boundary,
    huge,
]:
    arithmetic(value, 0)

# Adjacent products straddle both tagged bounds, including exactly -2**62.
# Swapping arguments exercises the asymmetric encoded multiplication formula.
root = 1 << 31
half = 1 << 61
for left, right in [
    (root - 1, root + 1),
    (root, root),
    (root + 1, root),
    (half, half - 1),
    (half, half),
]:
    for left_sign in [-1, 1]:
        for right_sign in [-1, 1]:
            arithmetic(left * left_sign, right * right_sign)
            if left != right:
                arithmetic(right * right_sign, left * left_sign)

transitions(boundary - 1, 1)
transitions(-boundary, -1)
transitions(0, huge)

for value in [0, boundary - 1, huge]:
    count(value, False)
    count(value, True)

for value in [-huge, 0, huge]:
    print(unwound_integer(value))


depth = 0
while sys._getframemodulename(depth) is not None:
    depth += 1
old_limit = sys.getrecursionlimit()
for number in [1, 1 << 100, 1]:
    for extra in (1, 2, 1):
        sys.setrecursionlimit(depth + extra)
        try:
            result = int(compare(number))
        except RecursionError:
            result = -9
        finally:
            sys.setrecursionlimit(old_limit)
        print(result)
