from datetime import date, datetime, timedelta, timezone


class ReflectedDate(date):
    def __eq__(self, other):
        return "reflected equality"

    def __gt__(self, other):
        return "reflected ordering"


class ChildDateTime(datetime):
    pass


def current():
    day = date(2024, 2, 29)
    earlier = date(2024, 2, 28)
    child = ReflectedDate(2024, 2, 29)
    west = timezone(timedelta(hours=-8))
    late = ChildDateTime(2024, 2, 29, 23, 59, tzinfo=west)
    for name, equal, before, after in (
        ("__eq__", True, False, False),
        ("__ne__", False, True, True),
        ("__lt__", False, True, False),
        ("__le__", True, True, False),
        ("__gt__", False, False, True),
        ("__ge__", True, False, True),
    ):
        descriptor = getattr(date, name)
        assert descriptor(day, child) is equal
        assert descriptor(earlier, day) is before
        assert descriptor(late, day) is equal
        assert descriptor(late, earlier) is after
        for other in (late, None, 1):
            assert getattr(day, name)(other) is NotImplemented
        assert getattr(datetime, name)(late, day) is NotImplemented
    assert (day == child) == "reflected equality"
    assert (day < child) == "reflected ordering"

    for call, expected in (
        (lambda: date.__le__(), "descriptor '__le__' of 'datetime.date' object needs an argument"),
        (lambda: date.__le__(None, b=day), "descriptor '__le__' requires a 'datetime.date' object but received a 'NoneType'"),
        (lambda: day.__le__(), "expected 1 argument, got 0"),
        (lambda: day.__le__(day, day), "expected 1 argument, got 2"),
        (lambda: day.__le__(b=day), "wrapper __le__() takes no keyword arguments"),
    ):
        try:
            call()
        except TypeError as error:
            assert str(error) == expected
        else:
            raise AssertionError("accepted invalid date descriptor arguments")
