import array
from array import array as array_type
from collections.abc import MutableMapping, MutableSequence, Sequence


def child(log: list[int]):
    log.append(1)
    return array.array


def parent(log: list[int]):
    log.append(2)
    return MutableSequence


print(array_type is array.array)
print(array_type is not list)
print(issubclass(array.array, MutableSequence))
print(issubclass(array.array, Sequence))
print(issubclass(array.array, MutableMapping))
log: list[int] = []
print(issubclass(child(log), parent(log)))
print(log)

# Use first-class values drawn from lists: the same operation must handle the
# virtual registration, builtin inheritance, and unrelated negative pairs.
classes = [bool, bytes, bytearray, complex, dict, float, frozenset, int, list,
           memoryview, object, range, set, slice, str, tuple, type, array_type,
           Sequence, MutableSequence, MutableMapping]
for subject in classes:
    print(subject.__module__)
    print(subject.__name__)
    print(subject.__qualname__)
    print(repr(subject))
    for ancestor in classes:
        print(issubclass(subject, ancestor))
