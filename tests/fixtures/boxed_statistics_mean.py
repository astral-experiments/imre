from decimal import Decimal
from fractions import Fraction
from statistics import StatisticsError, mean


def current():
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
    for values, error in [
        ([], StatisticsError),
        (["x"], TypeError),
        ([Decimal("0.1"), 1.0], TypeError),
        (Interrupted(), RuntimeError),
    ]:
        try:
            mean(values)
        except error as exc:
            print(type(exc).__name__, exc)
        else:
            raise AssertionError("invalid input accepted")
