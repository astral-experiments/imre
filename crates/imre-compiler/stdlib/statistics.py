"""Statistical functions from CPython 3.14.5 Lib/statistics.py."""
# See licenses/CPython.txt for the Python Software Foundation license.

import math
import numbers
from collections import namedtuple
from decimal import Decimal
from fractions import Fraction
from functools import reduce
from itertools import chain, count, groupby, islice, repeat
from math import fsum, isfinite, isinf, sqrt, sumprod
from operator import itemgetter

__all__ = ["StatisticsError", "correlation", "covariance", "harmonic_mean", "linear_regression", "mean", "median"]


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


def harmonic_mean(data, weights=None):
    """Return the harmonic mean of data.

    The harmonic mean is the reciprocal of the arithmetic mean of the
    reciprocals of the data.  It can be used for averaging ratios or
    rates, for example speeds.

    Suppose a car travels 40 km/hr for 5 km and then speeds-up to
    60 km/hr for another 5 km. What is the average speed?

        >>> harmonic_mean([40, 60])
        48.0

    Suppose a car travels 40 km/hr for 5 km, and when traffic clears,
    speeds-up to 60 km/hr for the remaining 30 km of the journey. What
    is the average speed?

        >>> harmonic_mean([40, 60], weights=[5, 30])
        56.0

    If ``data`` is empty, or any element is less than zero,
    ``harmonic_mean`` will raise ``StatisticsError``.

    """
    if iter(data) is data:
        data = list(data)

    errmsg = 'harmonic mean does not support negative values'

    n = len(data)
    if n < 1:
        raise StatisticsError('harmonic_mean requires at least one data point')
    elif n == 1 and weights is None:
        x = data[0]
        if isinstance(x, (numbers.Real, Decimal)):
            if x < 0:
                raise StatisticsError(errmsg)
            return x
        else:
            raise TypeError('unsupported type')

    if weights is None:
        weights = repeat(1, n)
        sum_weights = n
    else:
        if iter(weights) is weights:
            weights = list(weights)
        if len(weights) != n:
            raise StatisticsError('Number of weights does not match data size')
        _, sum_weights, _ = _sum(w for w in _fail_neg(weights, errmsg))

    try:
        data = _fail_neg(data, errmsg)
        T, total, count = _sum(w / x if w else 0 for w, x in zip(weights, data))
    except ZeroDivisionError:
        return 0

    if total <= 0:
        raise StatisticsError('Weighted sum must be positive')

    return _convert(sum_weights / total, T)


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


def covariance(x, y, /):
    """Covariance

    Return the sample covariance of two inputs *x* and *y*. Covariance
    is a measure of the joint variability of two inputs.

    >>> x = [1, 2, 3, 4, 5, 6, 7, 8, 9]
    >>> y = [1, 2, 3, 1, 2, 3, 1, 2, 3]
    >>> covariance(x, y)
    0.75
    >>> z = [9, 8, 7, 6, 5, 4, 3, 2, 1]
    >>> covariance(x, z)
    -7.5
    >>> covariance(z, x)
    -7.5

    """
    # https://en.wikipedia.org/wiki/Covariance
    n = len(x)
    if len(y) != n:
        raise StatisticsError('covariance requires that both inputs have same number of data points')
    if n < 2:
        raise StatisticsError('covariance requires at least two data points')
    xbar = fsum(x) / n
    ybar = fsum(y) / n
    sxy = sumprod((xi - xbar for xi in x), (yi - ybar for yi in y))
    return sxy / (n - 1)


def correlation(x, y, /, *, method='linear'):
    """Pearson's correlation coefficient

    Return the Pearson's correlation coefficient for two inputs. Pearson's
    correlation coefficient *r* takes values between -1 and +1. It measures
    the strength and direction of a linear relationship.

    >>> x = [1, 2, 3, 4, 5, 6, 7, 8, 9]
    >>> y = [9, 8, 7, 6, 5, 4, 3, 2, 1]
    >>> correlation(x, x)
    1.0
    >>> correlation(x, y)
    -1.0

    If *method* is "ranked", computes Spearman's rank correlation coefficient
    for two inputs.  The data is replaced by ranks.  Ties are averaged
    so that equal values receive the same rank.  The resulting coefficient
    measures the strength of a monotonic relationship.

    Spearman's rank correlation coefficient is appropriate for ordinal
    data or for continuous data that doesn't meet the linear proportion
    requirement for Pearson's correlation coefficient.

    """
    # https://en.wikipedia.org/wiki/Pearson_correlation_coefficient
    # https://en.wikipedia.org/wiki/Spearman%27s_rank_correlation_coefficient
    n = len(x)
    if len(y) != n:
        raise StatisticsError('correlation requires that both inputs have same number of data points')
    if n < 2:
        raise StatisticsError('correlation requires at least two data points')
    if method not in {'linear', 'ranked'}:
        raise ValueError(f'Unknown method: {method!r}')

    if method == 'ranked':
        start = (n - 1) / -2            # Center rankings around zero
        x = _rank(x, start=start)
        y = _rank(y, start=start)

    else:
        xbar = fsum(x) / n
        ybar = fsum(y) / n
        x = [xi - xbar for xi in x]
        y = [yi - ybar for yi in y]

    sxy = sumprod(x, y)
    sxx = sumprod(x, x)
    syy = sumprod(y, y)

    try:
        return sxy / _sqrtprod(sxx, syy)
    except ZeroDivisionError:
        raise StatisticsError('at least one of the inputs is constant')


LinearRegression = namedtuple('LinearRegression', ('slope', 'intercept'))


def linear_regression(x, y, /, *, proportional=False):
    """Slope and intercept for simple linear regression.

    Return the slope and intercept of simple linear regression
    parameters estimated using ordinary least squares. Simple linear
    regression describes relationship between an independent variable
    *x* and a dependent variable *y* in terms of a linear function:

        y = slope * x + intercept + noise

    where *slope* and *intercept* are the regression parameters that are
    estimated, and noise represents the variability of the data that was
    not explained by the linear regression (it is equal to the
    difference between predicted and actual values of the dependent
    variable).

    The parameters are returned as a named tuple.

    >>> x = [1, 2, 3, 4, 5]
    >>> noise = NormalDist().samples(5, seed=42)
    >>> y = [3 * x[i] + 2 + noise[i] for i in range(5)]
    >>> linear_regression(x, y)  #doctest: +ELLIPSIS
    LinearRegression(slope=3.17495..., intercept=1.00925...)

    If *proportional* is true, the independent variable *x* and the
    dependent variable *y* are assumed to be directly proportional.
    The data is fit to a line passing through the origin.

    Since the *intercept* will always be 0.0, the underlying linear
    function simplifies to:

        y = slope * x + noise

    >>> y = [3 * x[i] + noise[i] for i in range(5)]
    >>> linear_regression(x, y, proportional=True)  #doctest: +ELLIPSIS
    LinearRegression(slope=2.90475..., intercept=0.0)

    """
    # https://en.wikipedia.org/wiki/Simple_linear_regression
    n = len(x)
    if len(y) != n:
        raise StatisticsError('linear regression requires that both inputs have same number of data points')
    if n < 2:
        raise StatisticsError('linear regression requires at least two data points')

    if not proportional:
        xbar = fsum(x) / n
        ybar = fsum(y) / n
        x = [xi - xbar for xi in x]  # List because used three times below
        y = (yi - ybar for yi in y)  # Generator because only used once below

    sxy = sumprod(x, y) + 0.0        # Add zero to coerce result to a float
    sxx = sumprod(x, x)

    try:
        slope = sxy / sxx   # equivalent to:  covariance(x, y) / variance(x)
    except ZeroDivisionError:
        raise StatisticsError('x is constant')

    intercept = 0.0 if proportional else ybar - slope * xbar
    return LinearRegression(slope=slope, intercept=intercept)


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


def _fail_neg(values, errmsg='negative value'):
    """Iterate over values, failing if any are less than zero."""
    for x in values:
        if x < 0:
            raise StatisticsError(errmsg)
        yield x


def _rank(data, /, *, key=None, reverse=False, ties='average', start=1) -> list[float]:
    """Rank order a dataset. The lowest value has rank 1.

    Ties are averaged so that equal values receive the same rank:

        >>> data = [31, 56, 31, 25, 75, 18]
        >>> _rank(data)
        [3.5, 5.0, 3.5, 2.0, 6.0, 1.0]

    The operation is idempotent:

        >>> _rank([3.5, 5.0, 3.5, 2.0, 6.0, 1.0])
        [3.5, 5.0, 3.5, 2.0, 6.0, 1.0]

    It is possible to rank the data in reverse order so that the
    highest value has rank 1.  Also, a key-function can extract
    the field to be ranked:

        >>> goals = [('eagles', 45), ('bears', 48), ('lions', 44)]
        >>> _rank(goals, key=itemgetter(1), reverse=True)
        [2.0, 1.0, 3.0]

    Ranks are conventionally numbered starting from one; however,
    setting *start* to zero allows the ranks to be used as array indices:

        >>> prize = ['Gold', 'Silver', 'Bronze', 'Certificate']
        >>> scores = [8.1, 7.3, 9.4, 8.3]
        >>> [prize[int(i)] for i in _rank(scores, start=0, reverse=True)]
        ['Bronze', 'Certificate', 'Gold', 'Silver']

    """
    # If this function becomes public at some point, more thought
    # needs to be given to the signature.  A list of ints is
    # plausible when ties is "min" or "max".  When ties is "average",
    # either list[float] or list[Fraction] is plausible.

    # Default handling of ties matches scipy.stats.mstats.spearmanr.
    if ties != 'average':
        raise ValueError(f'Unknown tie resolution method: {ties!r}')
    if key is not None:
        data = map(key, data)
    val_pos = sorted(zip(data, count()), reverse=reverse)
    i = start - 1
    result = [0.0] * len(val_pos)
    for _, g in groupby(val_pos, key=itemgetter(0)):
        group = list(g)
        size = len(group)
        rank = i + (size + 1) / 2
        for value, orig_pos in group:
            result[orig_pos] = rank
        i += size
    return result


def _sqrtprod(x: float, y: float) -> float:
    "Return sqrt(x * y) computed with improved accuracy and without overflow/underflow."

    h = sqrt(x * y)

    if not isfinite(h):
        if isinf(h) and not isinf(x) and not isinf(y):
            # Finite inputs overflowed, so scale down, and recompute.
            scale = 2.0 ** -512  # sqrt(1 / sys.float_info.max)
            return _sqrtprod(scale * x, scale * y) / scale
        return h

    if not h:
        if x and y:
            # Non-zero inputs underflowed, so scale up, and recompute.
            # Scale:  1 / sqrt(sys.float_info.min * sys.float_info.epsilon)
            scale = 2.0 ** 537
            return _sqrtprod(scale * x, scale * y) / scale
        return h

    # Improve accuracy with a differential correction.
    # https://www.wolframalpha.com/input/?i=Maclaurin+series+sqrt%28h**2+%2B+x%29+at+x%3D0
    d = sumprod((x, h), (y, -h))
    return h + d / (2.0 * h)
