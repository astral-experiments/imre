import sys
from dataclasses import dataclass


@dataclass
class Cell:
    text: str
    number: int

    @property
    def next_number(self) -> int:
        self.number += 1
        return self.number


@dataclass
class Box:
    cell: Cell


def left_key(row: Box) -> tuple[str, int]:
    return (row.cell.text, row.cell.number)


def right_key(row: Cell) -> tuple[str, int]:
    return (row.text, row.number)


def compare(rows: list[Box], others: list[Cell]) -> bool:
    left = sorted(map(left_key, rows))
    right = sorted(map(right_key, others))
    for a, b in zip(left, right):
        if b != a:
            return False
    return True


def unequal(rows: list[Box], others: list[Cell]) -> bool:
    if len(rows) != len(others):
        return True
    left = sorted(map(left_key, rows))
    right = sorted(map(right_key, others))
    return any(b != a for a, b in zip(left, right))


def identity(i: int) -> int:
    return i


def integers(a: list[int], b: list[int]) -> bool:
    if len(a) != len(b):
        return False
    left = sorted(map(identity, a))
    right = sorted(map(identity, b))
    return all(x == y for x, y in zip(left, right))


def observe(row: Box) -> int:
    return row.cell.next_number


def observed(a: list[Box], b: list[Box]) -> bool:
    left = sorted(map(observe, a))
    right = sorted(map(observe, b))
    return all(x == y for x, y in zip(left, right))


def checked(row: Box) -> tuple[str, int]:
    print(row.cell.text)
    if row.cell.number < 0:
        raise ValueError("negative")
    return (row.cell.text, row.cell.number)


def checked_equal(a: list[Box], b: list[Box]) -> bool:
    left = sorted(map(checked, a))
    right = sorted(map(checked, b))
    return all(x == y for x, y in zip(left, right))


def nullable(rows: list[Box], others: list[Cell]) -> bool | None:
    left = sorted(map(left_key, rows))
    right = sorted(map(right_key, others))
    for a, b in zip(left, right):
        if a != b:
            return False
    return True


def label(value: bool | None) -> str:
    if value is None:
        return "None"
    return "True" if value else "False"


def bounded_projection(rows: list[Box], others: list[Cell]) -> None:
    depth = 0
    while sys._getframemodulename(depth) is not None:
        depth += 1
    old_limit = sys.getrecursionlimit()
    for extra in (1, 2, 1):
        sys.setrecursionlimit(depth + extra)
        try:
            result = int(compare(rows, others))
        except RecursionError:
            result = -9
        finally:
            sys.setrecursionlimit(old_limit)
        print(result)


big = 100000000000000000000000000000000000003
first = Cell("β", big)
second = Cell("λ", -big)
third = Cell("β", big + 1)
one = Box(first)
two = Box(second)
print(compare([], []))
print(compare([one], []))
print(compare([], [first]))
print(compare([one], [first]))
print(compare([one], [third]))
print(compare([one, two], [second, first]))
print(unequal([], [second]))
print(unequal([], []))
print(unequal([one], [first]))
print(unequal([one], [second]))
print(unequal([one, two], [second, first]))
print(integers([], []))
print(integers([big], [big + 0]))
print(integers([-big], []))
print(integers([big], [-big]))
print(integers([big, 0], [0, big]))
print(label(nullable([], [])))
print(label(nullable([one], [first])))
print(label(nullable([one], [second])))
print(observed([one], [one]))
print(first.number)
try:
    print(checked_equal([two], [one]))
except ValueError as error:
    print(str(error))

bounded_projection([], [])
bounded_projection([one], [first])
bounded_projection([one, two], [first, second])
