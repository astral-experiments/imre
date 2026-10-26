"""Native worker for the pinned CPython email date tests and real Werkzeug wrappers."""
import sys
from datetime import datetime, timedelta, timezone
from email.utils import format_datetime, formatdate, parsedate_to_datetime


def with_date() -> str:
    offset = sys.argv[9]
    tz: timezone | None = None
    if offset != "None":
        tz = timezone(timedelta(microseconds=int(offset)))
    date = datetime(int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5]),
                    int(sys.argv[6]), int(sys.argv[7]), int(sys.argv[8]), tzinfo=tz)
    return format_datetime(date, sys.argv[10] == "1")


def call() -> str:
    mode = sys.argv[1]
    if mode == "parse":
        return parsedate_to_datetime(sys.argv[2]).isoformat()
    if mode == "datetime":
        return with_date()
    local, gmt = sys.argv[3] == "1", sys.argv[4] == "1"
    if mode == "int":
        return formatdate(int(sys.argv[2]), local, gmt)
    if mode == "float":
        return formatdate(float(sys.argv[2]), local, gmt)
    return formatdate(None, local, gmt)


try:
    print(("ok", call()))
except ValueError as error:
    print(("ValueError", str(error)))
except OverflowError as error:
    print(("OverflowError", str(error)))
