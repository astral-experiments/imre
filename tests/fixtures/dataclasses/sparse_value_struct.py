from copy import deepcopy
from itertools import islice

from imre import struct
from imre.json import decode, encode


@struct
class Point:
    x: float
    y: bool


def scan(rows: list[Point]) -> float:
    total = 0.0
    for point in rows:
        total += point.x
    return total


def key(point: Point) -> int:
    return int(point.x)


def main() -> None:
    p = Point(-0.0, False)
    rows = [p] * 2048
    dense = [p for i in range(2048)]
    for i in [127, 0, 63, 64, 66, 65, 511, 2047]:
        q = Point(float(i + 1), True)
        rows[i] = q
        dense[i] = q
    saved = rows[64]
    alias = rows
    alias[64] = p
    dense[64] = p
    rows[127] = Point(0.0, False)
    dense[127] = Point(0.0, False)
    print(saved.x)
    print(rows[64].x)
    print(rows[127].x)
    print(rows == dense)
    print(dense == rows)
    empty = decode("[]", list[Point])
    print(rows == empty)
    print(empty == rows)
    for item in islice(rows, 3):
        print(item.x)
    print(scan(rows))
    print(scan(dense))
    print(Point(512.0, True) in rows)
    print(saved in rows)
    print(rows[::-3] == dense[::-3])
    print(rows[1::7] == dense[1::7])
    print(encode(rows[60:68]))
    clone = deepcopy(rows)
    twice = rows * 2
    rows.append(Point(12.0, True))
    print(rows.pop().x)
    print(rows.pop().x)
    print(len(clone))
    print(scan(clone))
    print(scan(twice))
    clone.sort(key=key)
    dense.sort(key=key)
    print(clone == dense)
    print(clone[-1].x)
    print(twice[2047].x)
    print(scan(rows))
    for i in range(1024):
        rows[i] = Point(float(i + 4), True)
    print(rows[0].x)
    print(rows[1023].x)
    print(rows[1024].x)
    print(twice[0].x)
    print(scan(rows))


main()
