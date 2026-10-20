def checked_sums(text: str) -> None:
    digits = list(map(int, text))
    # The length guard permits at most one term for these near-limit values.
    # Two terms reach the small-int boundary; three require BigInt fallback.
    print(sum([2305843009213693951 for digit in digits]))
    print(sum([-2305843009213693952 for digit in digits]))
    print(
        sum(
            [2305843009213693951 if digit else -2305843009213693952 for digit in digits]
        )
    )
    print(sum([2305843009213693951 for digit in digits[::-1]]))
    print(sum([-2305843009213693952 for digit in digits[-1::-2]]))


def early_continue() -> int:
    values = [1]
    cursor = 0
    pauses = 2
    total = 0
    while cursor < len(values):
        total += 2305843009213693951
        if pauses > 0:
            pauses -= 1
            continue
        cursor += 1
    return total


def stale_reverse_length() -> int:
    values = [1, 2, 3]
    alias = values
    cursor = len(values) + -1
    alias.pop()
    alias.pop()
    total = 0
    while cursor >= 0:
        total += 2305843009213693951
        cursor += -1
    return total


def rebound_reverse_source() -> int:
    values = [1, 2, 3]
    cursor = len(values) + -1
    values = [1]
    total = 0
    while cursor >= 0:
        total += 2305843009213693951
        cursor += -1
    return total + len(values)


def nonzero_start(text: str) -> int:
    digits = list(map(int, text))
    cursor = 0
    total = 1 << 100
    while cursor < len(digits):
        digit = digits[cursor]
        cursor += 1
        total += digit
    return total


def progress_before_continue(text: str) -> int:
    digits = list(map(int, text))
    cursor = 0
    total = 0
    while cursor < len(digits):
        digit = digits[cursor]
        cursor += 1
        if digit == 0:
            continue
        total += -digit
        if digit == 9:
            break
    else:
        total += 1 << 100
    return total


for text in ["", "0", "01", "012", "019"]:
    checked_sums(text)
    print(nonzero_start(text))
    print(progress_before_continue(text))
print(early_continue())
print(stale_reverse_length())
print(rebound_reverse_source())
