import json
from datetime import UTC, date, datetime as dt, time, timedelta, timezone


def test_http_date():
    assert UTC is timezone.utc
    assert timezone(timedelta(0)) is UTC
    # JSON and the system clock both supply a boxed float to the HTTP date path.
    stamp = dt.fromtimestamp(json.loads("1700000000.5625"), timezone.utc)
    assert stamp.tzinfo is timezone.utc and stamp.microsecond == 562500
    assert isinstance(stamp, dt) and isinstance(stamp, date)
    fields = stamp.timetuple()
    assert fields == (2023, 11, 14, 22, 13, 20, 1, 318, -1)
    assert (fields.tm_wday, fields.tm_yday, fields.tm_isdst) == (1, 318, -1)
    assert fields.tm_zone is None and fields.tm_gmtoff is None
    assert repr(fields).startswith("time.struct_time(tm_year=2023,")
    leap = date(2000, 2, 29)
    assert leap.timetuple() == (2000, 2, 29, 0, 0, 0, 1, 60, -1)
    assert leap.weekday() == 1 and leap.toordinal() == 730179
    print(str(stamp), repr(leap))


def test_cookie_timestamp():
    lifetime = timedelta(days=31)
    assert int(lifetime.total_seconds()) == 2678400
    assert str(lifetime.total_seconds()) == "2678400.0"
    before = dt.fromtimestamp(-1, tz=UTC)
    assert before == dt(1969, 12, 31, 23, 59, 59, tzinfo=UTC)
    assert dt.fromtimestamp(json.loads("-0.125"), UTC).microsecond == 875000
    assert dt.fromtimestamp(json.loads("0.9999999"), UTC) == dt.fromtimestamp(1, UTC)
    assert dt.fromtimestamp(json.loads("-0.0000005"), UTC) == dt.fromtimestamp(0, UTC)
    east = timezone(timedelta(hours=2), "EET")
    shifted = dt.fromtimestamp(timestamp=-1, tz=east)
    assert shifted.tzinfo is east and shifted.hour == 1 and shifted.day == 1
    assert shifted == before
    small = timedelta(microseconds=-1)
    assert (small.days, small.seconds, small.microseconds) == (-1, 86399, 999999)
    assert str(small.total_seconds()) == "-1e-06"
    print(str(before), str(shifted), int(lifetime.total_seconds()))


def test_time_of_day():
    midnight = time()
    assert type(midnight) is time and isinstance(midnight, time)
    assert (time.__module__, time.__name__, time.__qualname__) == ("datetime", "time", "time")
    assert not isinstance(midnight, date)
    assert midnight.tzinfo is None and midnight.fold == 0
    day = date(2000, 2, 29)
    # Werkzeug http_date combines a plain date with UTC midnight.
    assert dt.combine(day, midnight, tzinfo=UTC) == dt(2000, 2, 29, tzinfo=UTC)
    east = timezone(timedelta(hours=2), "east")
    clock = time(12, 34, second=56, microsecond=7, tzinfo=east, fold=1)
    assert (clock.hour, clock.minute, clock.second, clock.microsecond, clock.fold) == (12, 34, 56, 7, 1)
    assert clock.tzinfo is east
    combined = dt.combine(dt(2000, 2, 29, 23), clock)
    assert combined.tzinfo is east and combined.fold == 1 and combined.hour == 12
    assert dt.combine(day, clock, tzinfo=None).tzinfo is None
    assert clock == time(10, 34, 56, 7, UTC)
    assert clock > time(10, 34, 56, 6, UTC)
    assert clock != time(12, 34, 56, 7)
    try:
        clock < midnight
    except TypeError as error:
        print(str(error))
    else:
        raise AssertionError("ordered aware and naive times")
    class Hour:
        def __index__(self):
            return 17
    assert time(Hour()).hour == 17
    for args, keywords in (
        ((24,), {}), ((0, 60), {}), ((0, 0, -1), {}), ((0, 0, 0, 1000000), {}),
        ((), {"fold": 2}), ((0, 0, 0, 0, True), {}), ((0, 0, 0, 0, None, 1), {}),
        (("bad",), {}), ((1 << 100,), {}), ((1,), {"hour": 2}), ((), {"span": 1}),
        ((24, "bad"), {}), ((24,), {"tzinfo": True}),
    ):
        try:
            time(*args, **keywords)
        except (TypeError, ValueError, OverflowError) as error:
            print(str(error))
        else:
            raise AssertionError("invalid time accepted")
    print(str(midnight), repr(clock), str(clock), str(combined))


def current():
    test_time_of_day()
    test_http_date()
    test_cookie_timestamp()
    for timestamp in (None, "0", 1 << 100):
        try:
            dt.fromtimestamp(timestamp, UTC)
        except (TypeError, OverflowError) as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid timestamp accepted")
    try:
        dt.fromtimestamp(0, 1)
    except TypeError as exc:
        print(str(exc))
    else:
        raise AssertionError("invalid timezone accepted")


if __name__ == "__main__":
    current()
