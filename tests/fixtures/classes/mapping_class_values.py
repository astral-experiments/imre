from array import array
from collections.abc import Mapping, MutableMapping, MutableSequence, Sequence
from typing import Mapping as TypingMapping


def choose(log: list[int], number: int):
    log.append(number)
    return [array, MutableMapping, Mapping, list, dict][number]


def shadowed():
    Mapping = list
    print(issubclass(dict, Mapping))
    print(issubclass(Mapping, TypingMapping))


def invalid(log: list[int]) -> int:
    log.append(8)
    return 1


def parameter(Mapping):
    print(issubclass(dict, Mapping))
    print(issubclass(Mapping, TypingMapping))


def before_assignment():
    try:
        print(MutableMapping)
    except UnboundLocalError as exc:
        print(str(exc))
    MutableMapping = dict
    print(issubclass(list, MutableMapping))


before_assignment()
parameter(list)
shadowed()
print(issubclass(array, Mapping))
print(issubclass(MutableMapping, Mapping))
print(issubclass(Sequence, Mapping))
print(issubclass(MutableSequence, Mapping))
print(issubclass(array, TypingMapping))
print(issubclass(Mapping, MutableMapping))
print(Mapping.__module__)
print(Mapping.__qualname__)
print(repr(Mapping))
log: list[int] = []
for n in range(5):
    print(issubclass(choose(log, n), Mapping))
print(log)
classes = [array, MutableMapping, Mapping, list, dict]
for x in classes:
    for y in classes:
        print(issubclass(x, y))

values: dict[str, int] = {}
items: list[int] = []
print(isinstance(values, MutableMapping))
print(isinstance(items, MutableMapping))
print(isinstance(values, Mapping))
print(isinstance(items, Mapping))
print(choose(log, 2) is Mapping)
print(issubclass(choose(log, 4), choose(log, 2)))


try:
    print(issubclass(invalid(log), Mapping))
except TypeError as exc:
    print(str(exc))
print(log)
