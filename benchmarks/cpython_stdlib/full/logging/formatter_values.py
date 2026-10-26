"""Verify live defaults, record mutation, missing fields, padding and clock overrides."""
import logging
import time
from collections import Counter, defaultdict


def show_seconds(f: logging.Formatter, r: logging.LogRecord) -> None:
    try:
        print(f.formatTime(r, "%S"))
    except ValueError as error:
        print("ValueError " + str(error))
    except OverflowError as error:
        print("OverflowError " + str(error))
    except OSError as error:
        print("OSError " + repr(error.errno))


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
    counts: Counter[str] = Counter(["x"])
    groups: defaultdict[str, list[int]] = defaultdict(list)
    fields = logging.makeLogRecord({"msg": "é🦀", "extra": ("left", "right"),
                                    "names": {"x": "abc", "0": "string zero"},
                                    "numbers": {0: "zero"}, "words": ["first"],
                                    "ints": [42], "data": b"\xff", "0": "named zero",
                                    "counts": counts, "groups": groups})
    for fmt in ("{msg[1]}", "{extra[١]}", "{names[x][2]}", "{numbers[０]}",
                "{words[0]}", "{ints[0]}", "{data[0]}", "{names[0]}", "{extra[+1]}",
                "{words[2]}", "{numbers[9223372036854775808]}", "{0}", "{msg} {}",
                "{counts[missing]}", "{counts[0]}", "{groups[added]}"):
        try:
            print(logging.Formatter(fmt, style="{").format(fields))
        except ValueError as error:
            print("ValueError " + str(error))
        except IndexError as error:
            print("IndexError " + str(error))
        except TypeError as error:
            print("TypeError " + str(error))
    print(repr(groups))
    optional_fields("é", b"ab")
    optional_fields(None, "🦀π")
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
    for seconds in [2**53 + 1, 2**63 - 1, -(2**63) - 1, 67767976233532800]:
        show_seconds(f, logging.makeLogRecord({"created": seconds}))
    for stamp in [float("nan"), float("inf"), float(2**57), -0.1]:
        show_seconds(f, logging.makeLogRecord({"created": stamp}))
    show_seconds(f, logging.makeLogRecord({"created": True}))
    print(f.formatTime(logging.makeLogRecord({"created": None}), "literal"))
    print("value=%+06d" % (-13))
    try:
        time.sleep(-0.25)
    except ValueError as error:
        print(str(error))


def optional_fields(maybe: str | None, choice: str | bytes) -> None:
    record = logging.makeLogRecord({"maybe": maybe, "choice": choice})
    for fmt in ("{maybe[0]}", "{choice[1]}"):
        try:
            print(logging.Formatter(fmt, style="{").format(record))
        except TypeError as error:
            print("TypeError " + str(error))


main()
