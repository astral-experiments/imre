from imre import struct


@struct
class Sample:
    x: float


def samples(n: int, large: float, small: float) -> list[Sample]:
    rows: list[Sample] = []
    for i in range(n):
        odd = float(i & 1)
        high = float(i & 2) * 0.5
        x = (1.0 - odd) * (1.0 - 2.0 * high) * large + odd * (1.0 - high) * small
        rows.append(Sample(x))
    return rows


def fold(rows: list[Sample], initial: float, scale: float) -> float:
    total = initial
    for row in rows:
        total += row.x * scale
    return total


def check(n: int) -> None:
    exact = samples(n, 17.0, -3.0)
    rounded = samples(n, 9007199254740992.0, 1.0)
    print(fold(exact, -13.0, 1.0))
    print(fold(exact, 0.0, 0.1))
    print(fold(rounded, 1.0, 1.0))
    print(fold(exact, 9007199254740992.0, -1.0))
    print(fold(samples(n, 0.0, 0.0), -0.0, -1.0))


for n in [0, 3, 4, 7, 8, 9, 16, 19, 37]:
    check(n)
