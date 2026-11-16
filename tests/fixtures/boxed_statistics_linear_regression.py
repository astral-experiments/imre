from statistics import LinearRegression, StatisticsError, linear_regression


def current():
    line = linear_regression([1, 2, 3], [5, 7, 9])
    assert type(line) is LinearRegression
    assert line.slope == 2.0 and line.intercept == 3.0
    assert line == (2.0, 3.0)
    assert linear_regression([1, 2, 3], [5, 10, 15], proportional=True) == (5.0, 0.0)

    try:
        linear_regression([1, 1], [1, 2])
    except StatisticsError as exc:
        assert str(exc) == 'x is constant'
    else:
        raise AssertionError('accepted constant x')
