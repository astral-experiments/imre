from sys import argv
from time import time

from imre import struct


@struct
class Point:
    x: float
    y: float

    def norm2(self) -> float:
        return self.x * self.x + self.y * self.y


def seed(n: int, p: Point, dense: bool) -> list[Point]:
    if not dense:
        return [p] * n
    # A list built with ordinary appends already has a dense row buffer.
    rows: list[Point] = []
    for i in range(n):
        rows.append(p)
    return rows


def scan(rows: list[Point], scale: float) -> float:
    total = 0.0
    for p in rows:
        total += p.norm2() * scale
    return total


def probe(rows: list[Point], n: int, count: int) -> float:
    total = 0.0
    for i in range(count):
        at = (i * 100_003 + 7) % n
        total += rows[at].norm2()
    return total


def main(n: int, k: int, stride: int, dense: bool, passes: int, probes: int) -> None:
    p = Point(3.0, 4.0)
    q = Point(6.0, 8.0)
    start = time()
    rows = seed(n, p, dense)
    ready = time()
    for i in range(k - 1):
        at = (i * stride + n // 2) % n
        rows[at] = q
    before_last = time()
    if k > 0:
        rows[((k - 1) * stride + n // 2) % n] = q
    changed = time()
    total = 0.0
    for i in range(passes):
        total += scan(rows, float(i + 1))
    scanned = time()
    random_total = probe(rows, n, probes)
    done = time()
    print(len(rows))
    print(total)
    print(random_total)
    print(ready - start)
    print(changed - ready)
    print(changed - before_last)
    print(scanned - changed)
    print(done - scanned)


main(
    int(argv[1]),
    int(argv[2]),
    int(argv[3]),
    argv[4] == "packed",
    int(argv[5]),
    int(argv[6]),
)
