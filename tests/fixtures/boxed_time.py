import json
import time


def current():
    from select import select
    from time import ctime, monotonic, perf_counter, time_ns

    from types import SimpleNamespace
    for name in ("time", "monotonic", "perf_counter"):
        info = time.get_clock_info(name)
        assert type(info) is SimpleNamespace and isinstance(info.implementation, str)
        assert info.monotonic is (name != "time") and info.adjustable is (name == "time")
        assert 0 < info.resolution <= 1
    assert time.get_clock_info("monotonic") == time.get_clock_info("perf_counter")
    for name, expected in (("time_ns", ValueError), (b"time", TypeError)):
        try:
            time.get_clock_info(name)
        except expected:
            pass
        else:
            raise AssertionError("invalid clock name accepted")

    first = monotonic()
    assert type(first) is float and time.monotonic is monotonic
    sleep = time.sleep
    assert sleep(0) is None
    before_sleep = monotonic()
    assert sleep(0.01) is None
    assert monotonic() - before_sleep >= 0.009
    for arg, expected in ((-1, ValueError), (None, TypeError), (10**50, OverflowError)):
        try:
            sleep(arg)
        except expected:
            pass
        else:
            raise AssertionError("invalid sleep duration accepted")

    second = perf_counter()
    assert first <= second <= monotonic()
    select([], [], [], 0.02)
    assert time.perf_counter() - second >= 0.01
    ns = time_ns()
    assert time.time_ns is time_ns and type(ns) is int
    assert abs(ns / 1_000_000_000 - time.time()) < 60
    for clock in (monotonic, perf_counter, time_ns):
        for args, kwargs in (((1,), {}), ((), {"x": 1})):
            try:
                clock(*args, **kwargs)
            except TypeError as error:
                print(str(error))
            else:
                raise AssertionError("clock accepted arguments")

    now = time.time()
    assert now > 0 and "." in str(now)
    assert len(time.gmtime(now)) == len(time.localtime(now)) == 9
    assert time.gmtime().tm_isdst == time.gmtime(None).tm_isdst == 0
    assert isinstance(time.localtime(None), time.struct_time)
    assert ctime is time.ctime and type(ctime()) is type(ctime(None)) is str
    for stamp in (0, -1, json.loads("-0.125"), 951782400, 1699162200, 1699165800):
        print(ctime(stamp))
        for convert in (time.gmtime, time.localtime):
            t = convert(stamp)
            assert type(t) is time.struct_time and isinstance(t, tuple)
            assert tuple(t) == (t.tm_year, t.tm_mon, t.tm_mday, t.tm_hour, t.tm_min,
                                t.tm_sec, t.tm_wday, t.tm_yday, t.tm_isdst)
            print(tuple(t), t.tm_zone, t.tm_gmtoff)
    # Rounding a wide integer through float before conversion loses one second.
    wide = (1 << 53) + 1
    assert time.gmtime(wide).tm_sec == time.gmtime(wide - 1).tm_sec + 1

    class Index:
        def __index__(self):
            return -1

    class Seconds(float):
        def __index__(self):
            raise AssertionError("ctime read a float subclass through __index__")

    assert ctime(Index()) == ctime(Seconds(-0.5)) == ctime(-1)
    assert time.gmtime(Index()) == time.gmtime(-1)
    custom = time.struct_time(time.gmtime(0), {"tm_zone": "custom", "tm_gmtoff": 30})
    assert tuple(custom) == tuple(time.gmtime(0))
    assert custom.tm_zone == "custom" and custom.tm_gmtoff == 30
    for value in ("0", 1 << 100):
        try:
            time.localtime(value)
        except (TypeError, OverflowError) as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid timestamp accepted")
    local_timestamp()


def local_timestamp():
    from time import mktime, struct_time

    for stamp in (-1, 0, 951782400, 1699162200, 1699165800):
        record = time.localtime(stamp)
        assert mktime(record) == mktime(tuple(record)) == stamp
    for fields in ((2024, 3, 10, 2, 30, 0, 6, 70, -1),
                   (2024, 11, 3, 1, 30, 0, 6, 308, 1),
                   (2024, 11, 3, 1, 30, 0, 6, 308, 0)):
        print(mktime(struct_time(fields)), mktime(fields))

    calls = []

    class Index:
        def __init__(self, value):
            self.value = value

        def __index__(self):
            calls.append(self.value)
            return self.value

    fields = (1970, 1, 1, 0, 0, 0, 3, 1, -1)
    indexed = tuple([Index(value) for value in fields])
    assert mktime(struct_time(indexed + ("us\x00east", Index(123)))) == mktime(fields)
    assert calls == list(fields) + [123]
    for value, visited in (
        (42, []), (list(fields), []), (fields[:-1], []), (fields + (0,), []),
        ((Index(1 << 40),) + indexed[1:], [1 << 40]),
        (struct_time(indexed + (42, Index(123))), list(fields)),
        (struct_time(fields + (None, Index(1 << 100))), [1 << 100]),
    ):
        start = len(calls)
        try:
            mktime(value)
        except (TypeError, OverflowError) as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid local time accepted")
        assert calls[start:] == visited
    for args, kwargs in (((), {}), ((fields, fields), {}), ((), {"tuple": fields})):
        try:
            mktime(*args, **kwargs)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid mktime call accepted")


if __name__ == "__main__":
    current()
