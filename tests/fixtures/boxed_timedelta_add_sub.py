from datetime import date, timedelta


class Duration(timedelta):
    def __rsub__(self, other):
        return "reflected subclass"


def current():
    tick = timedelta.resolution
    two = timedelta(microseconds=2)
    neg_tick = timedelta(microseconds=-1)
    assert tick + neg_tick == timedelta()
    assert neg_tick.__radd__(two) == tick
    assert timedelta.__rsub__(two, neg_tick) == timedelta(microseconds=-3)
    assert two - tick == tick
    assert timedelta.max - timedelta.max == timedelta()
    assert timedelta(days=999999999) - timedelta.max == timedelta(days=-1, microseconds=1)
    assert type(Duration(days=1) + tick) is timedelta
    assert tick - Duration() == "reflected subclass"
    assert tick.__add__(date(2000, 1, 1)) is NotImplemented
    assert tick.__rsub__(1) is NotImplemented
    for operation, operand in ((timedelta.max.__add__, tick), (timedelta.min.__sub__, tick)):
        try:
            operation(operand)
        except OverflowError:
            pass
        else:
            raise AssertionError("accepted a timedelta outside the day limit")
