"""Execute the replace operations requested by CPython TestDateTime.test_replace."""
from datetime import datetime
import sys


def replace() -> datetime:
    original = datetime(
        int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]),
        int(sys.argv[5]), int(sys.argv[6]), int(sys.argv[7]), fold=int(sys.argv[8]),
    )
    name = sys.argv[9]
    value = int(sys.argv[10])
    if name == "":
        return original.replace()
    if name == "year":
        return original.replace(year=value)
    if name == "month":
        return original.replace(month=value)
    if name == "day":
        return original.replace(day=value)
    if name == "hour":
        return original.replace(hour=value)
    if name == "minute":
        return original.replace(minute=value)
    if name == "second":
        return original.replace(second=value)
    if name == "microsecond":
        return original.replace(microsecond=value)
    raise AssertionError("unmeasured replacement field")


try:
    dt = replace()
    print((dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second, dt.microsecond, dt.fold))
except ValueError as error:
    print(("ValueError", str(error)))
