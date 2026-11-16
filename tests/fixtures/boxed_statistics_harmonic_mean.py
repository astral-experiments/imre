from fractions import Fraction
from statistics import StatisticsError, harmonic_mean


def current():
    data = [Fraction(1, 3), Fraction(2, 3)]
    weights = [1, 3]
    assert harmonic_mean(iter(data), weights=iter(weights)) == Fraction(8, 15)
    assert harmonic_mean([0, -1]) == 0

    try:
        harmonic_mean([0, 2], weights=[1, -1])
    except StatisticsError as exc:
        assert str(exc) == "harmonic mean does not support negative values"
    else:
        raise AssertionError("accepted a negative weight")
