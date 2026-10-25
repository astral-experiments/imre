"""Verify live defaults, record mutation, missing fields, padding and clock overrides."""
import logging
import time


def main() -> None:
    defaults = {"route": "initial"}
    f = logging.Formatter("{route}:{message}", style="{", defaults=defaults)
    r = logging.makeLogRecord({"msg": "arrived"})
    try:
        print(f.formatMessage(r))
    except ValueError as error:
        print(str(error))
    print(f.format(r))
    defaults["route"] = "changed"
    print(f.format(r))
    r.route = "record"
    print(f.format(r))
    print(f.formatStack("first\nsecond"))
    r.stack_info = "one\ntwo"
    r.exc_text = "cached exception"
    print(f.format(r))
    r = logging.makeLogRecord({"msg": "ok", "created": 0.0, "msecs": 5.0})
    f = logging.Formatter("{asctime}.{msecs:03.0f} {message}", datefmt="%Y-%m-%d %H:%M:%S", style="{")
    f.converter = time.gmtime
    print(f.format(r))
    print(r.asctime)
    f.default_msec_format = None
    f.default_time_format = "%d/%m/%Y"
    print(f.formatTime(r))
    f.default_msec_format = "%s.%03d"
    print(f.formatTime(r))
    print("value=%+06d" % (-13))
    try:
        time.sleep(-0.25)
    except ValueError as error:
        print(str(error))


main()
