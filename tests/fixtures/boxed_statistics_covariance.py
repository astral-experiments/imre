from statistics import StatisticsError, covariance


def current():
    assert covariance([1, 2, 3], [10, 30, 20]) == 5.0
    assert covariance(range(1, 10), range(9, 0, -1)) == -7.5

    for x, y, message in [
        ([1], [], "covariance requires that both inputs have same number of data points"),
        ([1], [1], "covariance requires at least two data points"),
    ]:
        try:
            covariance(x, y)
        except StatisticsError as exc:
            assert str(exc) == message
        else:
            raise AssertionError("accepted invalid covariance inputs")
