import json
import time


def current():
    now = time.time()
    assert now > 0 and "." in str(now)
    assert len(time.gmtime(now)) == len(time.localtime(now)) == 9
    assert time.gmtime().tm_isdst == time.gmtime(None).tm_isdst == 0
    assert isinstance(time.localtime(None), time.struct_time)
    for stamp in (0, -1, json.loads("-0.125"), 951782400, 1699162200, 1699165800):
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
