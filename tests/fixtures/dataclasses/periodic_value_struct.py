from imre import struct


@struct
class Reading:
    x: float
    y: float


def readings(n: int, prefilled: bool) -> list[Reading]:
    rows: list[Reading] = []
    if prefilled:
        rows.append(Reading(-1.0, 9.0))
    i = -1
    x = -1.0
    for i in range(n):
        x = float(i & 5)
        rows.append(Reading(x, float(i % 8) - x))
    print(i)
    print(x)
    return rows


def key(row: Reading) -> int:
    return int(row.y)


def checksum(rows: list[Reading]) -> float:
    total = 0.0
    for row in rows:
        total = total * 2.0 + row.x - row.y
    return total


def main(n: int) -> None:
    rows = readings(n, False)
    original = readings(n, False)
    print(checksum(readings(n, True)))
    print(rows == original)
    print(Reading(5.0, 2.0) in rows)
    print(checksum(rows[::-2]))
    print(checksum(rows * 2))
    if n > 0:
        row = rows.pop()
        rows.append(row)
        print(rows == original)
        alias = rows
        rows[n // 2] = Reading(-7.0, -3.0)
        print(checksum(alias))
        print(checksum(original))
        original.sort(key=key)
        print(checksum(original))


for n in [0, 3, 8, 9, 10, 16, 19, 24]:
    main(n)
