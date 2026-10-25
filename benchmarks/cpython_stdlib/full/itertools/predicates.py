from io import StringIO
from itertools import dropwhile, filterfalse, takewhile
from typing import Iterator


class Probe:
    def __init__(self, calls: list[int]) -> None:
        self.calls = calls

    def below(self, value: int) -> int:
        self.calls.append(value)
        if value == 2:
            raise ValueError("refuse two")
        return int(value < 4)


class Reenter:
    def __init__(self, cursor: Iterator[int], trace: list[int]) -> None:
        self.cursor = cursor
        self.trace = trace

    def test(self, value: int) -> bool:
        self.trace.append(value)
        if value == 1:
            self.trace.append(next(self.cursor, -7))
        return value == 1 or value == 3


def pause(value: int) -> bool:
    if value == 2:
        raise StopIteration("paused")
    return value < 4


def chosen() -> int:
    print("evaluate predicate")
    return 3


def invalid_source() -> int:
    print("evaluate iterable")
    return 5


def blank(line: str) -> bool:
    return line == "\n"


values = [1, 2, 4, 1]
seen: list[int] = []
dropped = dropwhile(Probe(seen).below, values)
print(repr((iter(dropped) is dropped, seen)))
try:
    next(dropped)
except ValueError as exc:
    print(str(exc))
print(repr((seen, next(dropped), seen)))
values.append(2)
print(repr((list(dropped), next(dropped, -1), seen)))

seen_take: list[int] = []
source = iter([1, 2, 3, 4, 1])
taken = takewhile(Probe(seen_take).below, source)
print(next(taken))
try:
    next(taken)
except ValueError as exc:
    print(str(exc))
print(repr((next(taken), next(taken, -1), next(taken, -1), next(source), seen_take)))

seen_filter: list[int] = []
filtered = filterfalse(Probe(seen_filter).below, [2, 4, 5, 1])
try:
    next(filtered)
except ValueError as exc:
    print(str(exc))
print(repr((list(filtered), next(filtered, -1), seen_filter)))
print(list(filterfalse(None, [0, 1, 0])))
print(list(filterfalse(bool, ["", "x", ""])))
print(list(dropwhile(bool, [1, 0, 2])))
print(list(takewhile(bool, ["x", "", "y"])))

later = filterfalse(pause, [1, 2, 4])
try:
    next(later)
except StopIteration as exc:
    print(str(exc))
print(repr((next(later), next(later, -1))))
truncated = takewhile(pause, [1, 2, 3, 4])
print(repr((list(truncated), list(truncated), next(truncated, -1))))
resumed = dropwhile(pause, [1, 2, 4, 1])
print(repr((list(resumed), list(resumed))))

stream = StringIO("")
live = takewhile(bool, stream)
print(next(live, "eof"))
stream.write("one\n")
stream.seek(0)
print(repr(next(live)))
stream.close()
try:
    next(live)
except ValueError as exc:
    print(str(exc))
lines = StringIO("\nfirst\n\nlast")
line_cursor = dropwhile(blank, lines)
print(repr((next(line_cursor), lines.readline(), next(line_cursor), next(line_cursor, "eof"))))

bad = takewhile(None, [1, 2])
for _ in range(3):
    try:
        print(next(bad, -1))
    except TypeError as exc:
        print(str(exc))
try:
    filterfalse(chosen(), invalid_source())
except TypeError as exc:
    print(str(exc))

try:
    dropwhile(chosen(), invalid_source(), 7)
except TypeError as exc:
    print(str(exc))
try:
    takewhile(chosen(), iterable=invalid_source())
except TypeError as exc:
    print(str(exc))

traced: list[int] = []
owner = Reenter(iter([0]), traced)
outer = dropwhile(owner.test, [1, 2, 3, 4])
owner.cursor = outer
print(repr((list(outer), traced)))
owner.cursor = iter([0])
traced.clear()
outer = takewhile(owner.test, [1, 2, 3, 4])
owner.cursor = outer
print(repr((list(outer), traced)))
owner.cursor = iter([0])
traced.clear()
outer = filterfalse(owner.test, [1, 2, 3, 4])
owner.cursor = outer
print(repr((list(outer), traced)))
owner.cursor = iter([0])
