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


if __name__ == "__main__":
    current()
