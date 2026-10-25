"""Supplemental differential exercise; this is not an upstream test ID."""
from itertools import repeat
from operator import length_hint


def count(log: list[int]) -> int:
    log.append(1)
    return 3


def main() -> None:
    log: list[int] = []
    data: list[int] = []
    it = repeat(times=count(log), object=data)
    other = iter(it)
    next(it).append(7)
    print(f"{len(next(other))} {length_hint(it)}")
    for item in other:
        print(item[0])
    data.append(8)
    print(f"{len(next(it, data))} {len(log)}")
    for _ in range(2):
        try:
            next(other)
        except StopIteration:
            print("exhausted")
    infinite = repeat("x")
    print(f"{next(infinite)} {next(infinite)} {length_hint(infinite, -3)}")
    probe = repeat(4, 1)
    print(f"{next(probe, count(log))} {next(probe, count(log))} {len(log)}")
    try:
        repeat(0, 1 << 100)
    except OverflowError:
        print("overflow")


main()
