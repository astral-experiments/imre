from statistics import StatisticsError, correlation


def current():
    assert correlation([1, 2, 3], [10, 30, 20]) == 0.5
    assert correlation([1, 2, 2], [3, 1, 1], method='ranked') == -1.0
    for x, y, message in [
        ([], [], 'correlation requires at least two data points'),
        ([1, 2, 3], [1, 2], 'correlation requires that both inputs have same number of data points'),
        ([2, 2], [1, 2], 'at least one of the inputs is constant'),
    ]:
        try:
            correlation(x, y)
        except StatisticsError as exc:
            assert str(exc) == message
        else:
            raise AssertionError('accepted invalid correlation inputs')
