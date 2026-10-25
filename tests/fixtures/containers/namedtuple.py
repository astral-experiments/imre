import collections
from collections import namedtuple as make_tuple

Pair = make_tuple("Pair", "left, right")
Other = collections.namedtuple("Other", ("a", "b"))
Single = make_tuple("Dot", ["x"])
Zero = make_tuple("Zero", "")
Separators = make_tuple("Separators", "a\x1cb\035c\u001ed\N{UNIT SEPARATOR}e")


def fields(pair: Pair) -> str:
    return pair.right


def make_pair(log: list[int]) -> tuple[int, str]:
    log.append(1)
    return 7, "seven"


def empty_fields(log: list[int]) -> tuple[()]:
    log.append(4)
    return ()


def failing_fields(log: list[int]) -> tuple[int, str]:
    log.append(5)
    raise ValueError("cannot read fields")


def label(log: list[int], event: int) -> int:
    log.append(event)
    return event


def make(log: list[int]) -> Pair:
    return Pair(*make_pair(log))


log: list[int] = []
pair = make(log)
print(pair.left)
print(fields(pair))
print(pair[-1])
left, right = pair
print(left)
print(right)
print(repr(pair))
print(repr(Zero()))
print(repr(Single(1)))
print(repr(Separators(1, 2, 3, 4, e=5)))
print(isinstance(pair, Pair))
print(isinstance(pair, Other))
print(isinstance(pair, tuple))
print(pair == (7, "seven"))
print((7, "seven") == pair)
print(Other(7, "seven") == pair)
print(Other(7, "other") != pair)
print(repr(Pair(right=label(log, 2), left=label(log, 3))))
print(repr(make(log)._fields))
print(Pair.__name__)
print(repr(Pair.__match_args__))
print(repr(Zero(*empty_fields(log))))
try:
    Pair(*failing_fields(log), *empty_fields(log))
except ValueError as error:
    print(str(error))
print(repr(log))
