from datetime import datetime, timedelta, timezone


def number(label: str, n: int) -> int:
    print(label)
    return n


def convert(value: datetime, target: timezone) -> None:
    try:
        result = value.astimezone(tz=target)
        print((result.isoformat(), result.fold, result is value, result.tzinfo is target))
    except OverflowError as error:
        print(("OverflowError", str(error)))
    except ValueError as error:
        print(("ValueError", str(error)))


utc = timezone.utc
named_utc = timezone(timedelta(), "named zero")
east = timezone(timedelta(hours=14))
west = timezone(timedelta(hours=-10))
seconds = timezone(timedelta(seconds=11, microseconds=13), "odd offset")
original = datetime(2020, 2, 29, 23, 58, 59, 987654, tzinfo=seconds, fold=1)
copy = original.replace()
print((copy is original, copy == original, copy.fold, copy.tzinfo is seconds))
print(original.replace(2024, 3, 2, 0, 0, 0, 0, None, fold=0).isoformat())
print(original.replace(day=number("day", 1), year=number("year", 2000)).isoformat())
print(original.replace(tzinfo=None).fold)
print(original.replace(tzinfo=utc).tzinfo is utc)
print(original.replace(tzinfo=seconds) is original)
for n in [0, 10000, 2**31, 2**100]:
    try:
        print(original.replace(year=n))
    except ValueError as error:
        print(("ValueError", str(error)))
    except OverflowError as error:
        print(("OverflowError", str(error)))
try:
    print(original.replace(fold=2, day=42))
except ValueError as error:
    print(str(error))
try:
    print(original.replace(day=0, hour=2**100))
except OverflowError as error:
    print(str(error))
try:
    print(original.replace(year=2019))
except ValueError as error:
    print(str(error))
for value in [
    original,
    datetime(2000, 1, 1, tzinfo=utc, fold=1),
    datetime(2000, 1, 1, tzinfo=named_utc, fold=1),
    datetime(1, 1, 1, tzinfo=east),
    datetime(9999, 12, 31, 23, 59, 59, tzinfo=west),
    datetime(1, 1, 1),
    datetime(9999, 12, 31, 23, 59, 59),
]:
    for target in [utc, named_utc, east, west, seconds]:
        convert(value, target)
for y, m, d, h, mi in [
    (1901, 12, 13, 20, 45),
    (2014, 3, 9, 2, 30),
    (2014, 11, 2, 1, 30),
    (2014, 3, 30, 2, 30),
    (2014, 10, 26, 2, 30),
    (2017, 4, 2, 1, 45),
    (2017, 10, 1, 2, 15),
    (2011, 12, 30, 12, 0),
]:
    for fold in [0, 1]:
        value = datetime(y, m, d, h, mi, 0, 123, fold=fold)
        convert(value, utc)
        convert(value, seconds)
