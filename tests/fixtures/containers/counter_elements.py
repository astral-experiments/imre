from collections import Counter
from dataclasses import dataclass
from operator import length_hint


def changing(c: Counter[str]) -> None:
    it = c.elements()
    print(iter(it) is it)
    print(c.elements() is not it)
    print(length_hint(it, 9))
    print(next(it))
    c['a'] = 0
    c['b'] = 2
    print(list(it))
    print(next(it, 'done'))


def invalidated(c: Counter[str]) -> None:
    it = c.elements()
    c['new'] = 1
    try:
        print(next(it))
    except RuntimeError as error:
        print(str(error))
    print(next(it, 'done'))


def overflow(count: int) -> None:
    c = Counter({'before': 1, 'large': count, 'after': 1})
    it = c.elements()
    print(next(it))
    try:
        print(next(it))
    except OverflowError as error:
        print(str(error))
    print(next(it, 'done'))


@dataclass(frozen=True)
class Key:
    n: int


def owned() -> None:
    c: Counter[Key] = Counter()
    key = Key(17)
    c[key] = 3
    it = c.elements()
    print(next(it).n)
    del c[key]
    print(next(it).n)
    print(next(it).n)
    try:
        print(next(it))
    except RuntimeError as error:
        print(str(error))


def argument(c: Counter[str], label: str) -> Counter[str]:
    print(label)
    return c


def bad_calls(c: Counter[str]) -> None:
    try:
        Counter.elements()
    except TypeError as error:
        print(str(error))
    try:
        c.elements(argument(c, 'positional'))
    except TypeError as error:
        print(str(error))
    try:
        c.elements(self=argument(c, 'duplicate'))
    except TypeError as error:
        print(str(error))
    try:
        Counter.elements(c, wrong=argument(c, 'keyword'))
    except TypeError as error:
        print(str(error))


print(sorted(Counter('ABCABC').elements()))
print(list(Counter[int]().elements()))
print(list(Counter({7: -2, 4: 3, 0: 0, 9: 1}).elements()))
print(list(Counter.elements(Counter('abaa'))))
print(list(Counter.elements(self=Counter('xyx'))))
print(list(Counter.elements({'k': 2})))
changing(Counter(a=3, b=1))
invalidated(Counter(a=1))
overflow(1267650600228229401496703205376)
overflow(-1267650600228229401496703205376)
owned()
bad_calls(Counter('abc'))
