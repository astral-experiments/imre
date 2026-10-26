from datetime import date, datetime, time, timedelta, timezone


def inspect_date(value: date | None) -> None:
    if value is None:
        print("absent")
        return
    print((value.year, value.month, value.day))
    print((isinstance(value, date), isinstance(value, datetime)))
    print((str(value), repr(value), value.isoformat()))
    print(f"{value}|{value!s}|{value!r}")
    if isinstance(value, datetime):
        print(value.hour)


def compare(left: date, right: date) -> None:
    print((left is right, left == right, left != right))
    try:
        print(left < right)
    except TypeError as exc:
        print(str(exc))
    try:
        print(left <= right)
    except TypeError as exc:
        print(str(exc))
    try:
        print(left > right)
    except TypeError as exc:
        print(str(exc))
    try:
        print(left >= right)
    except TypeError as exc:
        print(str(exc))


def keep(value: date | datetime) -> date:
    return value


def widen(value: datetime | None) -> date | None:
    return value


def trace(label: str, n: int) -> int:
    print(label)
    return n


d = date(2000, 2, 29)
other = date(2000, 2, 29)
n = datetime(2000, 2, 29, 23, 30)
east = timezone(timedelta(hours=5, minutes=45))
a = datetime(2000, 2, 29, 23, 30, tzinfo=east)
inspect_date(None)
inspect_date(date(True, 1, 2))
inspect_date(d)
inspect_date(n)
inspect_date(a)
print((keep(d) is d, keep(n) is n))
print((widen(n) is n, widen(None) is None))
compare(d, d)
compare(d, other)
compare(d, date(2000, 3, 1))
compare(d, n)
compare(n, d)
compare(n, n)
compare(n, a)
print(datetime.combine(d, time(), tzinfo=timezone.utc).isoformat())
print(datetime.combine(time=time(8, 9, 10, 11, fold=1), date=a, tzinfo=east))
c = datetime.combine(a, time(19, 7, 3, 100, fold=1))
print((c.fold, c.tzinfo is None, c.isoformat()))
print(datetime.combine(d, time(), tzinfo=east).tzinfo is east)
print(datetime.combine(d, time()) is datetime.combine(d, time()))
for delta in [timedelta(), timedelta(microseconds=-1), timedelta(hours=25)]:
    print((str(d + delta), str(delta + d), str(d - delta), (d + delta) is d))
for year, month, day in [
    (1, 1, 1),
    (9999, 12, 31),
    (1900, 2, 29),
    (0, 12, 0),
    (10000, 1, 1),
    (2000, 13, 0),
    (2000, 1, 32),
    (2**31, 1, 1),
    (0, 2**100, 1),
    (1, -(2**100), 1),
]:
    try:
        print(date(year, month, day))
    except ValueError as exc:
        print(str(exc))
    except OverflowError as exc:
        print(str(exc))
try:
    print(date(day=trace("day", 0), month=trace("month", 0), year=trace("year", 0)))
except ValueError as exc:
    print(str(exc))
try:
    print(date(9999, 12, 31) + timedelta(days=1))
except OverflowError as exc:
    print(str(exc))
try:
    print(date(1, 1, 1) + timedelta(microseconds=-1))
except OverflowError as exc:
    print(str(exc))
