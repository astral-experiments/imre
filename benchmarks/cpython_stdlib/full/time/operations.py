"""POSIX mktime, struct_time and local timezone behavior observable by a caller."""
from enum import StrEnum
from http import HTTPStatus
from time import gmtime, localtime, mktime, struct_time


class Zone(StrEnum):
    UTC = "UTC"


class Indexed:
    value: int

    def __init__(self, value: int):
        self.value = value

    def __index__(self) -> int:
        print(self.value)
        return self.value


class Reject:
    def __index__(self) -> int:
        print("reject")
        raise ValueError("index failed")


def from_list(fields: list[int]) -> None:
    value = struct_time(fields)
    print(len(value))
    print(value.tm_year)
    print(value.tm_zone)
    print(value.tm_gmtoff)


def timestamp(value: struct_time | float) -> float:
    if isinstance(value, struct_time):
        value = mktime(value)
    return value


def indexed_offset(value: Indexed | None) -> float:
    return mktime(struct_time((1970, 1, 1, 0, 0, 0, 0, 1, -1, "UTC", value)))


print(struct_time.__module__)
print(struct_time.__name__)
print(struct_time.n_fields)
print(struct_time.n_sequence_fields)
print(struct_time.n_unnamed_fields)
print(struct_time.__match_args__)
for stamp in (-2, -1, 0, 1, 1710054000, 1730611800, 1730615400):
    value = localtime(stamp)
    print(isinstance(value, struct_time))
    print(isinstance(value, tuple))
    print(isinstance(tuple(value), struct_time))
    print(tuple(value))
    print(value.tm_zone)
    print(value.tm_gmtoff)
    print(timestamp(value))
    print(timestamp(float(stamp)))
    print(gmtime(stamp))
assert gmtime(HTTPStatus.OK) == gmtime(200)
assert localtime(Indexed(200)) == localtime(200)
for stamp in (2**53 + 1, -(2**53) - 1):
    try:
        print((gmtime(stamp).tm_sec, localtime(stamp).tm_sec))
    except (OSError, OverflowError) as exc:
        print(str(exc))
for dst in (0, 1, -1):
    for month, day, hour in ((3, 10, 2), (11, 3, 1)):
        civil = (2024, month, day, hour, 30, 0, 6, 1, dst)
        normalized = mktime(civil)
        print(normalized)
        print(localtime(normalized))
print(mktime((2024, 13, 0, 25, -1, 62, 30, -5, -1)))
print(mktime((Indexed(1970), True, Indexed(1), 0, 0, 0, 0, 0, -1)))
try:
    mktime((Indexed(1970), Reject(), Indexed(1), 0, 0, 0, 0, 0, -1))
except ValueError as exc:
    print(str(exc))
try:
    mktime((2147483648, Indexed(3), 1, 0, 0, 0, 0, 0, -1))
except OverflowError as exc:
    print(str(exc))
try:
    mktime((-2147483648, 1, 1, 0, 0, 0, 0, 0, -1))
except OverflowError as exc:
    print(str(exc))
try:
    mktime((1970, "1", 1, 0, 0, 0, 0, 0, -1))
except TypeError as exc:
    print(str(exc))
try:
    mktime((1, 2, 3))
except TypeError as exc:
    print(str(exc))
try:
    mktime([1970, 1, 1, 0, 0, 0, 0, 0, -1])
except TypeError as exc:
    print(str(exc))
try:
    struct_time((1, 2))
except TypeError as exc:
    print(str(exc))
try:
    struct_time(12)
except TypeError as exc:
    print(str(exc))
for data in ([1970,1,1,0,0,0,0,0,-1], [1970,1,1,0,0,0,0,0,-1,13],
             [1970,1,1,0,0,0,0,0,-1,13,17], [1,2], [1,2,3,4,5,6,7,8,9,10,11,12]):
    try:
        from_list(data)
    except TypeError as exc:
        print(str(exc))

extended = struct_time([1970, 1, 1, 0, 0, 0, 0, 0, -1, "custom", 123])
print(len(extended))
print(extended[:])
print(repr(extended))
print(extended.tm_zone)
print(extended.tm_gmtoff)
print(mktime(extended))
print(struct_time(extended).tm_zone)
enum_zone = struct_time((1970, 1, 1, 0, 0, 0, 0, 0, -1, Zone.UTC, 0))
assert enum_zone.tm_zone is Zone.UTC
assert mktime(enum_zone) == mktime(struct_time((1970, 1, 1, 0, 0, 0, 0, 0, -1, "UTC", 0)))
bad_zone = struct_time((1970, 1, 1, 0, 0, 0, 0, 0, -1, 3))
print(bad_zone.tm_zone)
try:
    mktime(bad_zone)
except TypeError as exc:
    print(str(exc))
bad_offset = struct_time((Indexed(1970), 1, 1, 0, 0, 0, 0, 0, -1, "UTC", "bad"))
print(bad_offset.tm_gmtoff)
try:
    mktime(bad_offset)
except TypeError as exc:
    print(str(exc))
try:
    mktime(struct_time((1970, 1, 1, 0, 0, 0, 0, 0, -1, "UTC", 2**100)))
except OverflowError as exc:
    print(str(exc))
print(indexed_offset(None))
enum_offset = struct_time((1970, 1, 1, 0, 0, 0, 0, 1, -1, "UTC", HTTPStatus.OK))
assert mktime(enum_offset) == mktime(struct_time((1970, 1, 1, 0, 0, 0, 0, 1, -1, "UTC", 200)))
for n in (0, -3600, 1 << 100):
    try:
        print(indexed_offset(Indexed(n)))
    except OverflowError as exc:
        print(str(exc))
try:
    mktime(struct_time((1970, 1, 1, 0, 0, 0, 0, 1, -1, "UTC", Reject())))
except ValueError as exc:
    print(str(exc))
try:
    mktime(struct_time((-2147483648, 1, 1, 0, 0, 0, 0, 1, -1, "UTC", Reject())))
except OverflowError as exc:
    print(str(exc))
try:
    mktime(struct_time((1970, 1, 1, 0, 0, 0, 0, 1, -1, 2, Reject())))
except TypeError as exc:
    print(str(exc))
# Run this last: glibc's mktime on an extreme year may affect its private offset guess.
try:
    print(mktime((2147483647, 2147483647, 1, 0, 0, 0, 0, 0, -1)))
except OverflowError as exc:
    print(str(exc))
