import email.utils
from datetime import datetime, timezone
from email import utils
from email.utils import format_datetime, formatdate, parsedate_to_datetime


def current():
    print(email.utils is utils, utils.formatdate is formatdate)
    print(email.__package__, utils.__package__, utils.__name__, hasattr(email, "__path__"))
    for value in (0, 0.0, -0.0000006, 0.9999996, 1666800000):
        print(formatdate(timeval=value, usegmt=True), formatdate(value))
    for text in ("Thu, 01 Jan 1970 00:00:00 GMT", "Thu, 01 Jan 1970 00:00:00 -0000",
                 "Fri, 02 Jan 1970 03:04:05 +0530", "28-Feb-70 12:34:56 EST"):
        dt = parsedate_to_datetime(data=text)
        print(isinstance(dt, datetime), dt.tzinfo is timezone.utc, dt.tzinfo is None)
        print(format_datetime(dt), format_datetime(dt, usegmt=True) if dt.tzinfo is timezone.utc else dt.hour)
    try:
        format_datetime(datetime(2000, 2, 29), usegmt=True)
    except ValueError as error:
        print(str(error))
    try:
        parsedate_to_datetime("not a date")
    except ValueError as error:
        print(str(error))
