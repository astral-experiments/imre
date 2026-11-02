import os
import time


def current():
    saved = os.environ.get("TZ")
    rules = [
        ("UTC0", ("UTC", "UTC"), 0, 0),
        ("ABC4DEF,M3.2.0/2,M11.1.0/2", ("ABC", "DEF"), 14400, 10800),
        ("NZST-12NZDT,M9.5.0/2,M4.1.0/3", ("NZST", "NZDT"), -43200, -46800),
    ]
    try:
        for rule, names, standard, summer in rules:
            previous = time.tzname
            os.environ["TZ"] = rule
            assert time.tzname is previous
            assert time.tzset() is None
            assert time.tzname == names and time.tzname is not previous
            assert time.timezone == standard and time.altzone == summer
            assert type(time.daylight) is int and time.daylight == (standard != summer)
    finally:
        if saved is None:
            del os.environ["TZ"]
        else:
            os.environ["TZ"] = saved
        time.tzset()
