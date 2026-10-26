import sys
from typing import Callable


def convert(value: str) -> int:
    print("convert " + value)
    return int(value)


def callback() -> Callable[[str], int]:
    print("callback")
    return convert


def source() -> list[str]:
    print("source")
    return ["2", "3"]


def start() -> int:
    print("start")
    return 10


def label(value: int) -> str:
    print(value)
    return "keep" if value % 2 else ""


def consume(fn: Callable[[str], int], values: list[str]) -> int:
    return sum(map(fn, values))


def inferred(value):
    return int(value)


class Changes:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values

    def append(self, value: int) -> int:
        if value == 1:
            self.values.append(3)
            self.values[1] = 20
        return value

    def pop(self, value: int) -> bool:
        self.values.pop()
        return value > 0


def frame_item(value: int) -> int:
    caller = sys._getframemodulename(1)
    if caller is None:
        return 0
    return value + 1


def prefix_count(names: list[str]) -> int:
    return len(list(item for item in names if item.startswith('x')))


def generator_frames() -> None:
    depth = 0
    while sys._getframemodulename(depth) is not None:
        depth += 1
    old_limit = sys.getrecursionlimit()
    for extra in (1, 2, 1):
        sys.setrecursionlimit(depth + extra)
        try:
            total = sum(frame_item(item) for item in [1, 2])
        except RecursionError:
            total = -9
        finally:
            sys.setrecursionlimit(old_limit)
        print(total)
        sys.setrecursionlimit(depth + extra)
        try:
            count = prefix_count(['x', 'y'])
        except RecursionError:
            count = -9
        finally:
            sys.setrecursionlimit(old_limit)
        print(count)
    print(any(frame_item(item) > 0 for item in [1, 2]))


def main() -> None:
    print(sum(map(callback(), source()), start()))
    print(consume(convert, ["4", "5"]))
    print(sum(map(inferred, ["6", "7"])))
    print(sum(map(lambda value: value * value, range(4))))
    print(sum(filter(lambda value: value % 2, range(6))))
    print(sum(map(lambda value: value + 1, filter(label, [0, 1, 2, 3]))))
    print(sum(value for value in filter(None, map(convert, ["0", "2", "3"]))))
    print(any(map(convert, ["1", "bad"])))
    print(all(map(convert, ["0", "bad"])))
    print(any(filter(None, map(convert, ["0", "1", "bad"]))))
    print(all(filter(lambda value: True, map(convert, ["0", "bad"]))))
    for text in filter(None, ["", "Ω", "", "last"]):
        print(text)
    nullable: list[int | None] = [None, 0, 3, None, -4]
    for value in filter(None, nullable):
        if value is not None:
            print(value)
    for floating in filter(None, [0.0, -0.0, 1.25]):
        print(floating)
    print(len(list(map(lambda value: value + 1, range(0)))))
    print(len(list(filter(label, range(0)))))
    values = [1, 2]
    print(sum(map(Changes(values).append, values)))
    values = [1, 2, 3, 4]
    print(sum(filter(Changes(values).pop, values)))
    print(len(values))
    # A strict mismatch is observable only if the consumer requests that pull.
    print(any(map(lambda pair: pair[0] > 0, zip([1, 2], [3], strict=True))))
    try:
        print(sum(map(lambda pair: pair[0], zip([1, 2], [3], strict=True))))
    except ValueError as error:
        print(error)
    try:
        print(len(list(filter(lambda value: int(value) > 0, ["1", "bad", "later"]))))
    except ValueError:
        print("predicate failed")


main()
generator_frames()
