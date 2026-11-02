from imre import struct


@struct
class Sample:
    x: float
    y: float


def fold(rows: list[Sample], initial: float, scale: float) -> float:
    total = initial
    for row in rows:
        total += (row.x - row.y) * scale
    return total


def exercise(n: int, a: float, b: float, initial: float) -> None:
    base = Sample(a, 0.0)
    other = Sample(b, 0.0)
    rows = [base] * n
    alias = rows
    for i in range(1, n, 3):
        rows[i] = other
    for i in [0, 63, 64, 65, n - 1]:
        alias[i] = other
        alias[i] = other
    alias[63] = base
    print(fold(rows, initial, 1.0))
    print(fold(rows, initial, 0.1))
    print(fold(rows[::-1], initial, 1.0))
    print(fold(rows[1::2], initial, 1.0))
    print(rows[-1].x)
    print(rows.pop().x)
    rows.append(base)
    print(fold(rows * 2, initial, 1.0))
    alias[64] = Sample(3.0, -2.0)
    print(fold(rows, initial, 1.0))
    print(rows[63].x)
    print(rows[64].y)
    print(rows[65].x)


for size in [66, 127, 128, 129, 257]:
    exercise(size, 7.0, -11.0, -13.0)
    exercise(size, 9007199254740992.0, -9007199254740992.0, 1.0)
    exercise(size, 0.1, -0.5, -0.0)
    exercise(size, -0.0, 0.0, -0.0)
