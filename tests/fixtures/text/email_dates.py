from datetime import datetime, timedelta, timezone
from email.utils import format_datetime, formatdate, parsedate_to_datetime


def parse(value: str) -> None:
    try:
        dt = parsedate_to_datetime(value)
        print((dt.isoformat(), format_datetime(dt)))
    except ValueError as error:
        print(str(error))
    except OverflowError as error:
        print(str(error))


def stamp(value: float) -> None:
    try:
        print((formatdate(value), formatdate(value, usegmt=True), formatdate(value, localtime=True, usegmt=True)))
    except ValueError as error:
        print(str(error))
    except OverflowError as error:
        print(str(error))


for value in ("Sun, 23 Sep 2001 20:10:55 -0700", "Sun,23 Sep 2001 20:10:55 +0000",
              "23 September 2001 20:10 -0000", "Friday, 19-Nov-82 16:14:55 EST",
              "Wed Feb  5 13:47:26 2003", "5 Feb 2003 13.47.26", "5 Feb 2003 13.47+0800",
              "05 Feb 68 13:47 GMT extra", "05 Feb 69 13:47", "05 Feb 2003 13:47 XYZ",
              "05\x1cFeb 20\u0663\u0664 13:47 +0000", "", "0", "A Complete Waste of Time",
              "05 Feb 2003 12.34.56.78+0800", "Tue, 06 Jun 2017 27:39:33 +0600",
              "Tue, 06 Jun 2017 07:39:33 +2600", "29 Feb 2023 00:00 GMT",
              "17 June , 2022", "Friday, -Nov-82 16:14:55 EST"):
    parse(value)
print(format_datetime(datetime(1, 1, 1)))
print(format_datetime(datetime(9999, 12, 31, 23, 59, 59, tzinfo=timezone.utc), usegmt=True))
print(format_datetime(datetime(2020, 12, 31, tzinfo=timezone(timedelta(seconds=-90, microseconds=3)))))
for date in (datetime(2001, 1, 7), datetime(2001, 1, 7, tzinfo=timezone(timedelta(hours=2)))):
    try:
        print(format_datetime(date, usegmt=True))
    except ValueError as error:
        print(str(error))
for ts in (-0.0000005, -1.9999995, 0.0, 0.9999995, 1000000000.5, 1609459199.9999998,
           1710053999.0, 1710054000.0, 1730613599.0, 1730613600.0):
    stamp(ts)
try:
    print(formatdate(2**127))
except OverflowError as error:
    print(str(error))
print(formatdate(0, True))
print(formatdate(0, False, True))
print(formatdate().endswith(" -0000"))
