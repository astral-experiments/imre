import array as arrays
import collections
from array import array as array_type
from collections import deque as deque_type
from collections.abc import MutableSequence, Sequence


def shadow(deque: int, array: str):
    print(deque)
    print(array)


def facts(left: bool, right: bool):
    print(left)
    print(right)


def evaluated(items: list[int]) -> list[int]:
    items.append(42)
    return items


def optional(items: list[str] | None):
    facts(isinstance(items, MutableSequence), isinstance(items, Sequence))


def text(value: str | bytes):
    facts(isinstance(value, MutableSequence), isinstance(value, Sequence))


classes = [tuple, str, bytes, list, bytearray, deque_type, array_type]
for subject in classes:
    print(subject.__module__)
    print(subject.__name__)
    facts(issubclass(subject, MutableSequence), issubclass(subject, Sequence))
facts(collections.deque is deque_type, deque_type is not arrays.array)
shadow(7, 'local')

ints: list[int] = []
strings: list[str] = ['x']
queue: deque_type[str] = deque_type(strings)
values: dict[int, int] = {1: 2}
print(isinstance(evaluated(ints), MutableSequence))
print(ints)
facts(isinstance(strings, MutableSequence), isinstance(strings, Sequence))
facts(isinstance(queue, MutableSequence), isinstance(queue, Sequence))
facts(isinstance((), MutableSequence), isinstance((), Sequence))
facts(isinstance((1, 2), Sequence), isinstance((1, 2), deque_type))
facts(isinstance(values, Sequence), isinstance(42, MutableSequence))
facts(isinstance(deque_type, Sequence), isinstance(queue, (str, deque_type)))
facts(isinstance(queue, collections.deque), isinstance(strings, deque_type))
optional(strings)
optional(None)
text("text")
text(b"bytes")
