"""Arithmetic mean and median from CPython 3.14.5 Lib/statistics.py."""
# See licenses/CPython.txt for the Python Software Foundation license.

import math
from decimal import Decimal
from fractions import Fraction
from functools import reduce
from itertools import chain, groupby, islice

__all__ = ["StatisticsError", "mean", "median"]


class StatisticsError(ValueError):
    pass


def mean(data):
    """Return the sample arithmetic mean of data.

    >>> mean([1, 2, 3, 4, 4])
    2.8

    >>> from fractions import Fraction as F
    >>> mean([F(3, 7), F(1, 21), F(5, 3), F(1, 3)])
    Fraction(13, 21)

    >>> from decimal import Decimal as D
    >>> mean([D("0.5"), D("0.75"), D("0.625"), D("0.375")])
    Decimal('0.5625')

    If ``data`` is empty, StatisticsError will be raised.

    """
    if type(data) is list or type(data) is tuple:
        partials = {}
        count = 0
        for value in data:
            if type(value) is not float or not math.isfinite(value):
                break
            n, d = value.as_integer_ratio()
            partials[d] = partials.get(d, 0) + n
            count += 1
        else:
            if count:
                return _binary_float_mean(partials, count)

        if data and type(value) is int:
            return _mean_from_integer(data, value, count, partials)

    T, total, n = _sum(data)
    if n < 1:
        raise StatisticsError('mean requires at least one data point')
    return _convert(total / n, T)


def median(data):
    """Return the median (middle value) of numeric data.

    When the number of data points is odd, return the middle data point.
    When the number of data points is even, the median is interpolated by
    taking the average of the two middle values:

    >>> median([1, 3, 5])
    3
    >>> median([1, 3, 5, 7])
    4.0

    """
    data = sorted(data)
    n = len(data)
    if n == 0:
        raise StatisticsError("no median for empty data")
    if n % 2 == 1:
        return data[n // 2]
    else:
        i = n // 2
        return (data[i - 1] + data[i]) / 2


def _mean_from_integer(data, value, count, partials):
    """Resume after the first exact integer in a builtin sequence."""
    # The direct float loop stopped on this integer. Resume immediately
    # after it without repeating the exact float arithmetic.
    values = iter(data)
    if count:
        next(islice(values, count, None), None)
    else:
        next(values)
    integer_sum = value
    count += 1
    for value in values:
        kind = type(value)
        if kind is int:
            integer_sum += value
        elif kind is float and math.isfinite(value):
            n, d = value.as_integer_ratio()
            partials[d] = partials.get(d, 0) + n
        else:
            if not partials and type(kind) is type:
                # Fold the exact integer prefix into one input. It represents
                # count values in the mean, and the tail iterator stays live.
                T, total, n = _sum(chain((integer_sum, value), values))
                return _convert(total / (n + count - 1), T)
            break
        count += 1
    else:
        if not partials:
            quotient, remainder = divmod(integer_sum, count)
            if remainder:
                return integer_sum / count
            return quotient
        partials[1] = partials.get(1, 0) + integer_sum
        return _binary_float_mean(partials, count)

    T, total, n = _sum(data)
    if n < 1:
        raise StatisticsError('mean requires at least one data point')
    return _convert(total / n, T)


def _binary_float_mean(partials, count):
    """Combine power-of-two denominators exactly and round only the mean."""
    denominator = max(partials)
    numerator = sum(n * (denominator // d) for d, n in partials.items())
    # Integer true division rounds the exact ratio to binary64 directly.
    return numerator / (denominator * count)


def _sum(data):
    """_sum(data) -> (type, sum, count)

    Return a high-precision sum of the given numeric data as a fraction,
    together with the type to be converted to and the count of items.

    Examples
    --------

    >>> _sum([3, 2.25, 4.5, -0.5, 0.25])
    (<class 'float'>, Fraction(19, 2), 5)

    Some sources of round-off error will be avoided:

    # Built-in sum returns zero.
    >>> _sum([1e50, 1, -1e50] * 1000)
    (<class 'float'>, Fraction(1000, 1), 3000)

    Fractions and Decimals are also supported:

    >>> from fractions import Fraction as F
    >>> _sum([F(2, 3), F(7, 5), F(1, 4), F(5, 6)])
    (<class 'fractions.Fraction'>, Fraction(63, 20), 4)

    >>> from decimal import Decimal as D
    >>> data = [D("0.1375"), D("0.2108"), D("0.3061"), D("0.0419")]
    >>> _sum(data)
    (<class 'decimal.Decimal'>, Fraction(6963, 10000), 4)

    Mixed types are currently treated as an error, except that int is
    allowed.

    """
    if type(data) is range and data:
        first, last = data[0], data[-1]
        # The mathematical length can exceed the limit of len(range(...)).
        size = (last - first) // data.step + 1
        total = (first + last) * size // 2
        return int, Fraction(total), size

    count = 0
    types = set()
    types_add = types.add
    partials = {}
    partials_get = partials.get

    for typ, values in groupby(data, type):
        types_add(typ)
        for n, d in map(_exact_ratio, values):
            count += 1
            partials[d] = partials_get(d, 0) + n

    if None in partials:
        # The sum will be a NAN or INF. We can ignore all the finite
        # partials, and just look at this special one.
        total = partials[None]
        assert not _isfinite(total)
    else:
        # Sum all the partial sums using builtin sum.
        total = sum(Fraction(n, d) for d, n in partials.items())

    T = reduce(_coerce, types, int)  # or raise TypeError
    return (T, total, count)


def _isfinite(x):
    try:
        return x.is_finite()  # Likely a Decimal.
    except AttributeError:
        return math.isfinite(x)


def _coerce(T, S):
    """Coerce types T and S to a common type, or raise TypeError.

    Coercion rules are currently an implementation detail. See the CoerceTest
    test class in test_statistics for details.

    """
    # See http://bugs.python.org/issue24068.
    assert T is not bool, "initial type T is bool"
    # If the types are the same, no need to coerce anything. Put this
    # first, so that the usual case (no coercion needed) happens as soon
    # as possible.
    if T is S:  return T
    # Mixed int & other coerce to the other type.
    if S is int or S is bool:  return T
    if T is int:  return S
    # If one is a (strict) subclass of the other, coerce to the subclass.
    if issubclass(S, T):  return S
    if issubclass(T, S):  return T
    # Ints coerce to the other type.
    if issubclass(T, int):  return S
    if issubclass(S, int):  return T
    # Mixed fraction & float coerces to float (or float subclass).
    if issubclass(T, Fraction) and issubclass(S, float):
        return S
    if issubclass(T, float) and issubclass(S, Fraction):
        return T
    # Any other combination is disallowed.
    msg = "don't know how to coerce %s and %s"
    raise TypeError(msg % (T.__name__, S.__name__))


def _exact_ratio(x):
    """Return Real number x to exact (numerator, denominator) pair.

    >>> _exact_ratio(0.25)
    (1, 4)

    x is expected to be an int, Fraction, Decimal or float.

    """
    try:
        return x.as_integer_ratio()
    except AttributeError:
        pass
    except (OverflowError, ValueError):
        # float NAN or INF.
        assert not _isfinite(x)
        return (x, None)

    try:
        # x may be an Integral ABC.
        return (x.numerator, x.denominator)
    except AttributeError:
        msg = f"can't convert type '{type(x).__name__}' to numerator/denominator"
        raise TypeError(msg)


def _convert(value, T):
    """Convert value to given numeric type T."""
    if type(value) is T:
        # This covers the cases where T is Fraction, or where value is
        # a NAN or INF (Decimal or float).
        return value

    if issubclass(T, int) and value.denominator != 1:
        T = float

    try:
        # FIXME: what do we do if this overflows?
        return T(value)
    except TypeError:
        if issubclass(T, Decimal):
            return T(value.numerator) / T(value.denominator)
        else:
            raise
