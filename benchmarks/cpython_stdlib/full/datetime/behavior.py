from datetime import UTC, datetime, timedelta, timezone

trace: list[str] = []


def component(name: str, value: int) -> int:
    trace.append(name)
    return value


def earlier(left: datetime, right: datetime) -> bool:
    return left < right


for n in [-3, -2, -1, 0, 1, 2, 3]:
    d = timedelta(microseconds=n, milliseconds=0.0005)
    print(d)
    print(d.total_seconds())
print(bool(timedelta(0)))
print(bool(timedelta(microseconds=-1)))
print(timedelta(days=999999999, seconds=37001, microseconds=123456).total_seconds())
print(timedelta(days=10**40, weeks=-(10**40 // 7), hours=-24 * (10**40 % 7)))
zero = timedelta()
named = timezone(zero, "UTC")
print(timezone(zero) is UTC)
print(named is UTC)
print(named == UTC)
print(named.utcoffset(None) is zero)
east = timezone(timedelta(hours=5, minutes=30, seconds=1, microseconds=4))
print(east.tzname(None))
aware = datetime(2020, 2, 29, 23, 30, 55, 987654, tzinfo=east, fold=1)
print(str(aware))
print(repr(aware))
print(aware.tzinfo is east)
print(aware.utcoffset() == east.utcoffset(aware))
print(aware.fold)
print((aware + timedelta()).fold)
naive = datetime(2020, 2, 29)
print(naive.tzinfo is None)
print(naive.utcoffset() is None)
print(naive == aware)
try:
    print(earlier(naive, aware))
except TypeError as exc:
    print(str(exc))
for year, month, day in [
    (2000, 2, 29),
    (1900, 2, 29),
    (0, 1, 1),
    (9999, 13, 2),
    (2**31, 1, 1),
    (2**100, 1, 1),
]:
    try:
        print(datetime(year, month, day))
    except ValueError as exc:
        print(str(exc))
    except OverflowError as exc:
        print(str(exc))
for n in [1000000000, -(10**20)]:
    try:
        print(timedelta(days=n))
    except OverflowError as exc:
        print(str(exc))
for d in [timedelta(days=1), timedelta(days=-1), timedelta(hours=-23, seconds=-1)]:
    try:
        print(timezone(d))
    except ValueError as exc:
        print(str(exc))
try:
    print(datetime(9999, 12, 31) + timedelta(days=1))
except OverflowError as exc:
    print(str(exc))
try:
    print(datetime(1, 1, 1) - timedelta(microseconds=1))
except OverflowError as exc:
    print(str(exc))
try:
    print(
        datetime(
            day=component("day", 55),
            month=component("month", 13),
            year=component("year", 0),
        )
    )
except ValueError as exc:
    print(str(exc))
print(trace)
