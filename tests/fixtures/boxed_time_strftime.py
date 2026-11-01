def current():
    import os
    import sys
    import time
    from time import gmtime, localtime, strftime, struct_time

    # CPython test.support checks for this extension during module initialization.
    has_strftime_extensions = False
    if sys.platform != "win32":
        try:
            has_strftime_extensions = time.strftime("%4Y") != "%4Y"
        except ValueError:
            pass
    print(has_strftime_extensions)
    before = strftime("%4Y", localtime())
    year = strftime("%4Y")
    assert year in (before, strftime("%4Y", localtime()))
    fields = (1987, 5, 23, 12, 3, 61, 5, 143, 1)
    assert strftime("%Y-%m-%d %H:%M:%S %w %j", fields) == "1987-05-23 12:03:61 6 143"
    assert strftime("%m/%d %w %j", (1, 0, 0, 0, 0, 0, -1, 0, -42)) == "01/01 0 001"
    record = struct_time(fields + ("Practice", 3600))
    print(strftime("%Z %z", record))
    print(strftime("%z", gmtime(0)))
    print(repr(strftime("%6Z", struct_time(fields + ("å", 0)))))
    assert strftime("%Y\x00%d/🐍\ud800%M", record) == "1987\x0023/🐍\ud80003"
    class Clock(tuple):
        def __iter__(self):
            raise AssertionError("time tuple conversion called __iter__")

    assert strftime("%Y %j", Clock(fields)) == "1987 143"

    zone_record = struct_time(fields + ("Åé", 0))
    original_zone = strftime("%Z", zone_record)
    previous_locale = os.environ.get("LC_ALL")
    try:
        for requested in ("C", "C.UTF-8"):
            os.environ["LC_ALL"] = requested
            assert strftime("%Z", zone_record) == original_zone
    finally:
        if previous_locale is None:
            del os.environ["LC_ALL"]
        else:
            os.environ["LC_ALL"] = previous_locale

    print(len(strftime("%0800Y", fields)))
    assert strftime("", fields) == ""

    class Index:
        def __index__(self):
            return 2

    assert strftime("%m", (1987, Index()) + fields[2:]) == "02"
    for values in ((), (None,), (b"%Y",), ("%Y", None), ("%Y", fields[:-1]),
                   ("%Y", list(fields)), ("%Y", fields, fields)):
        try:
            strftime(*values)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid strftime call accepted")
    try:
        strftime(format="%Y")
    except TypeError as exc:
        print(str(exc))
    else:
        raise AssertionError("strftime accepted keyword arguments")
    for field, value in ((0, -(1 << 31)), (1, 13), (2, -1), (3, 24), (4, 60),
                         (5, 62), (6, -2), (7, 367)):
        try:
            strftime("%Y", fields[:field] + (value,) + fields[field + 1:])
        except (ValueError, OverflowError) as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid time fields accepted")


if __name__ == "__main__":
    current()
