from imre import struct
from sys import argv
from time import time


@struct
class Point:
    x: float
    y: float

    def norm2(self) -> float:
        return self.x * self.x + self.y * self.y


def build(n: int) -> list[Point]:
    rows: list[Point] = []
    for i in range(n):
        x = float(i & 1023)
        rows.append(Point(x, x + 1.0))
    return rows


def scan(rows: list[Point], scale: float) -> float:
    total = 0.0
    for point in rows:
        total += point.norm2() * scale
    return total


def main(n: int, passes: int, shared: bool) -> None:
    t0 = time()
    rows = [Point(3.0, 4.0)] * n if shared else build(n)
    t1 = time()
    result = 0.0
    for i in range(passes):
        result += scan(rows, float(i + 1))
    t2 = time()
    print(len(rows))
    print(result)
    print(t1 - t0)
    print(t2 - t1)


main(int(argv[1]), int(argv[2]), argv[3] == "shared")
