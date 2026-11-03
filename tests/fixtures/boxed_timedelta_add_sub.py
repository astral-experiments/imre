from datetime import UTC, date, datetime, timedelta, timezone


class Duration(timedelta):
    def __rsub__(self, other):
        return "reflected subclass"


def test_datetime_shift():
    east = timezone(timedelta(hours=2))
    start = datetime(2024, 2, 29, 23, 59, 59, 999999, tzinfo=east, fold=1)
    tick = timedelta.resolution
    end = start + tick
    assert end == datetime(2024, 3, 1, tzinfo=east)
    assert end.tzinfo is east and end.fold == 0
    assert tick + start == end
    assert end - tick == start and end - start == tick
    assert end - datetime(2024, 2, 29, 22, tzinfo=UTC) == timedelta(0)
    assert datetime.__rsub__(end, start) == -tick
    assert start.__sub__(object()) is NotImplemented
    assert start.__rsub__(tick) is NotImplemented
    for wrong, kind in ((None, "NoneType"), (date(2000, 1, 1), "datetime.date")):
        try:
            datetime.__add__(wrong, tick)
        except TypeError as exc:
            assert str(exc) == (
                "descriptor '__add__' requires a 'datetime.datetime' object "
                f"but received a '{kind}'"
            )
        else:
            raise AssertionError("accepted the wrong datetime descriptor receiver")
    for dt, step in [(datetime.min, -tick), (datetime.max, tick)]:
        try:
            dt + step
        except OverflowError:
            pass
        else:
            raise AssertionError("accepted an out-of-range shift")
    try:
        start - datetime(2024, 3, 1)
    except TypeError:
        pass
    else:
        raise AssertionError("mixed aware and naive datetimes")

    class Child(datetime):
        pass

    class Reflected:
        def __radd__(self, other):
            return other

    later = Child(2001, 2, 3) + timedelta(days=2)
    assert type(later) is Child and later == datetime(2001, 2, 5)
    assert start + Reflected() is start


def test_date_arithmetic():
    events = []

    class Child(date):
        def __new__(cls, *args):
            events.append(("new", args))
            return date.__new__(cls, *args)

        def __init__(self, *args):
            events.append(("init", args))

        def __rsub__(self, other):
            return "reflected"

    start = Child(2000, 2, 29)
    events.clear()
    # A negative microsecond has a normalized day field of -1.
    step = timedelta(microseconds=-1)
    for moved in [start + step, step + start, start - step]:
        assert type(moved) is Child
    assert events == [
        (hook, ymd)
        for ymd in [(2000, 2, 28), (2000, 2, 28), (2000, 3, 1)]
        for hook in ("new", "init")
    ]
    later = date(2000, 3, 1)
    assert later - start == "reflected"
    assert date.__rsub__(start, later) == timedelta(1)
    assert start - later == timedelta(-1)
    assert date.max - date.min == timedelta(days=3652058)
    assert date.__rsub__(start, step) is NotImplemented
    dt = datetime(2000, 2, 29)
    for method in (date.__add__, date.__radd__, date.__sub__, date.__rsub__):
        assert method(start, dt) is NotImplemented
        assert method(dt, step) is NotImplemented
    for operation in [date.min.__add__, date.max.__sub__]:
        try:
            operation(step)
        except OverflowError:
            pass
        else:
            raise AssertionError("accepted an out-of-range date")


def current():
    test_date_arithmetic()
    test_datetime_shift()
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
