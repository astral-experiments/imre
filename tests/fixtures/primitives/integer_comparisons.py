"""Integer comparison fast paths preserve arbitrary precision and evaluation order."""


def delta(left: int, right: int) -> int:
    return left - right


def total(left: int, right: int) -> int:
    return left + right


def distance(left: int, right: int) -> int:
    gap = left - right
    return -gap if gap < 0 else gap


def comparisons(left: int, right: int, threshold: int) -> None:
    print(delta(left, right) < threshold)
    print(delta(left, right) <= threshold)
    print(delta(left, right) > threshold)
    print(delta(left, right) >= threshold)
    print(delta(left, right) == threshold)
    print(delta(left, right) != threshold)
    print(threshold < delta(left, right))
    print(threshold <= delta(left, right))
    print(threshold > delta(left, right))
    print(threshold >= delta(left, right))
    print(threshold == delta(left, right))
    print(threshold != delta(left, right))
    print(total(left, right) < threshold)
    print(distance(left, right) <= threshold)


def local_versions(left: int, right: int) -> int:
    current = left - right
    earlier = current
    current = left + right
    return earlier


def observed(label: str, value: int) -> int:
    print(label)
    return value


def captured_owner(seed: int) -> None:
    # The fresh heap result has no owner other than this slot and the captured
    # first argument when evaluating the second argument replaces the slot.
    value = seed + (1 << 150)
    print(delta(value, (value := 0)) < 0)
    print(value)


def rebound_threshold(threshold: int) -> None:
    print(delta((threshold := threshold + 1), 0) == threshold)
    print(threshold)
    print(delta(threshold, (threshold := threshold + 2)) < threshold)
    print(threshold)


def keyword_binding(value: int) -> None:
    print(delta(right=(value := value + 1), left=value) == 0)
    print(value)


def failing_argument() -> int:
    print("argument failure")
    raise ValueError("argument")


def effectful_delta(left: int, right: int) -> int:
    print("callee")
    return left - right


def quotient(left: int, right: int) -> int:
    return left // right


def replaced_parameter(left: int, right: int) -> int:
    left = right
    return left


def unsafe_total(left: int, right: int) -> int:
    return left + right + left


def unsafe_negation(left: int, right: int) -> int:
    return -(left + right)


def unsafe_distance(left: int, right: int) -> int:
    value = left + right
    return -value if value < 0 else value


def product(left: int, right: int) -> int:
    return left * right


def floating_delta(left: float, right: float) -> float:
    return left - right


def escaping_delta(left: int, right: int) -> int:
    result = delta(left, right)
    left = 0
    right = 0
    return result


boundary = 1 << 62
huge = 1 << 150
for left, right, threshold in [
    (0, 0, 0),
    (1, 0, 0),
    (-1, 0, 0),
    (-boundary, boundary - 1, 0),
    (boundary - 1, -boundary, boundary - 1),
    (-boundary, 0, -boundary),
    (boundary - 1, 0, boundary - 1),
    (-boundary, -boundary, -boundary),
    (boundary - 1, boundary - 1, boundary - 1),
    (0, 0, huge),
    (huge, huge, 0),
    (huge, -huge, huge),
    (-huge, huge, -huge),
]:
    comparisons(left, right, threshold)

for value in [5, huge]:
    print(delta(right=observed("right", 2), left=observed("left", value)) < value)
for seed in [-2 * huge, -huge, 0]:
    captured_owner(seed)
for threshold in [-3, 0, -huge, huge]:
    rebound_threshold(threshold)
for value in [5, huge]:
    keyword_binding(value)

try:
    print(delta(observed("first", huge), failing_argument()) < 0)
except ValueError:
    print("caught later argument")
try:
    print(delta(failing_argument(), observed("unreachable", huge)) < 0)
except ValueError:
    print("caught first argument")
print(effectful_delta(huge, huge) == 0)
print(replaced_parameter(huge, 0) == 0)
try:
    print(quotient(huge, 0) < 0)
except ZeroDivisionError:
    print("caught callee")

for left, right in [(-boundary, -boundary), (boundary - 1, boundary - 1), (huge, -huge)]:
    print(unsafe_total(left, right) < 0)
    print(unsafe_negation(left, right) > 0)
    print(unsafe_distance(left, right) >= 0)
    print(product(left, right) > 0)
for number in [-0.0, 1.5, float("inf"), float("nan")]:
    print(floating_delta(number, 0.0) <= 0.0)
    print(floating_delta(number, number) == 0.0)
for left, right in [(-boundary, boundary - 1), (huge, -huge)]:
    print(escaping_delta(left, right))

for left, right in [(1, 1), (0, 1), (-boundary, -boundary)]:
    print(local_versions(left, right) == 0)
