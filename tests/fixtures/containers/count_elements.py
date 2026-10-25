import _collections
import collections
from _collections import _count_elements as native_count
from collections import Counter, _count_elements
from collections import _count_elements as tally_keys


class Audit:
    counts: dict[str, int]
    trace: list[str]

    def __init__(self):
        self.counts = {}
        self.trace = []

    def get(self, key: str, default: int) -> int:
        self.trace.append('get ' + key)
        if key == 'bad get':
            raise ValueError(key)
        return self.counts.get(key, default)

    def __setitem__(self, key: str, count: int) -> None:
        self.trace.append('set ' + key)
        if key == 'bad set':
            raise ValueError(key)
        self.counts[key] = count


class Biased(Audit):
    def get(self, key: str, default: int) -> int:
        self.trace.append('biased ' + key)
        return self.counts.get(key, default) + 2


def tally(mapping: Audit, keys: list[str]) -> None:
    tally_keys(mapping, keys)


def target(markers: list[int], counts: dict[int, int]) -> dict[int, int]:
    markers.append(10)
    return counts


def source(markers: list[int]) -> list[int]:
    markers.append(20)
    return [2, 1, 3, 4]


def pull(value: int, markers: list[int]) -> int:
    markers.append(value)
    if value == 3:
        raise ValueError('third pull')
    return value


def rhs(markers: list[int]) -> int:
    markers.append(1)
    return 8


def receiver(markers: list[int], value: Audit) -> Audit:
    markers.append(2)
    return value


def index(markers: list[int]) -> str:
    markers.append(3)
    return 'assigned'


counts: dict[str, int] = {'z': -3, '£': 999999999999999999999999}
print(_count_elements(counts, 'a£aba'))
print(counts)
print(native_count(counts, {'b': 200, 'z': 10}))
print(counts)
collections._count_elements(counts, counts)
print(counts)
print(_collections._count_elements({}, 'hello'))
counter = Counter('aba')
native_count(counter, 'bc')
print(counter)

numbers: dict[int, int] = {}
markers: list[int] = []
try:
    _count_elements(target(markers, numbers), (pull(k, markers) for k in source(markers)))
except ValueError as error:
    print(error)
print(markers)
print(numbers)

mapping = Audit()
tally(mapping, ['a', 'b', 'a'])
print(mapping.counts)
print(mapping.trace)
for bad in ['bad get', 'bad set']:
    try:
        tally(mapping, ['before', bad, 'after'])
    except ValueError as error:
        print(error)
print(mapping.counts)
print(mapping.trace)
derived = Biased()
tally(derived, ['x', 'x'])
print(derived.counts)
print(derived.trace)

order: list[int] = []
receiver(order, mapping)[index(order)] = rhs(order)
print(order)
print(mapping.counts['assigned'])
try:
    _count_elements(None, 3)
except TypeError as error:
    print(error)
try:
    _count_elements(None, [])
except AttributeError as error:
    print(error)
try:
    _count_elements({})
except TypeError as error:
    print(error)
try:
    _count_elements(mapping={}, iterable=[])
except TypeError as error:
    print(error)
