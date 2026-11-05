from decimal import Decimal
from fractions import Fraction
from statistics import StatisticsError, mean


def current():
    huge = 1 << 500
    assert mean((huge, huge)) == huge
    assert type(mean([huge, -huge])) is int
    assert mean([0, 1, 1]) == 2 / 3

    class Interrupted:
        def __iter__(self):
            yield 1
            raise RuntimeError("iteration interrupted")

    samples = [
        [1, 2, 3],
        [1, 2],
        [True, False],
        [1, 2.0],
        [1e50, 1.0, -1e50],
        [2**300, 2**300],
        [Fraction(1, 3), Fraction(2, 5)],
        [Decimal("0.5"), Decimal("0.75")],
        [float("inf")],
        [float("inf"), float("-inf")],
    ]
    for values in samples:
        result = mean(x for x in values)
        print(type(result).__name__, result)
    smallest = float.fromhex("0x0.0000000000001p-1022")
    largest = float.fromhex("0x1.fffffffffffffp+1023")
    halfway = float.fromhex("0x1.0000000000001p+0")
    for values in [
        [largest, largest],
        [-largest, 1.0, largest],
        [smallest, 0.0],
        [-smallest, 0.0],
        [1.0, halfway],
        [0.0, -0.0],
        [1.5, float("inf")],
        [1.5, 2],
    ]:
        print(mean(values).hex(), mean(tuple(values)).hex(), mean(iter(values)).hex())

    for values, error in [
        ([], StatisticsError),
        (["x"], TypeError),
        (Interrupted(), RuntimeError),
    ]:
        try:
            mean(values)
        except error as exc:
            print(type(exc).__name__, exc)
        else:
            raise AssertionError("invalid input accepted")

    try:
        mean([Decimal("0.1"), 1.0])
    except TypeError as exc:
        # The types come from a set, so CPython can name them in either order.
        assert str(exc) in (
            "don't know how to coerce Decimal and float",
            "don't know how to coerce float and Decimal",
        )
    else:
        raise AssertionError("mixed Decimal and float accepted")
