"""Exercise the argument display in threading_helper.run_concurrently."""
from typing import Callable


def mark(value: int) -> int:
    print(repr(('field', value)))
    return value


def mutate(values: list[int]) -> int:
    print(repr(('append', 9)))
    values.append(9)
    return 7


def captures(values: list[int]) -> tuple[int, ...]:
    return (mark(1), *values, mutate(values), *values, mark(2))


def call_prefix(func: Callable[[int], int], args: list[int]) -> None:
    packed = (func, *args)
    print(repr((len(packed), packed[0](len(packed)))))
    if args:
        print(packed[1])


class Worker:
    def __init__(self, base: int) -> None:
        self.base = base

    def add(self, value: int) -> int:
        return self.base + value


def tail_marker(args: list[int]) -> None:
    packed = ('first', *args, 'last')
    print(repr((packed[0], len(packed), packed[-1])))


def checked(value: int) -> int:
    print(repr(('pull', value)))
    if value == 2:
        raise ValueError('failed during iteration')
    return value


def midway(args: list[int]) -> None:
    try:
        capture = (mark(4), *(checked(i) for i in args), mark(5))
        print(capture)
    except ValueError as exc:
        print(str(exc))


def bad(args: int) -> None:
    try:
        capture = (mark(6), *args, mark(8))
        print(capture)
    except TypeError as exc:
        print(str(exc))


def nested(args: list[list[int]]) -> None:
    packed = (*args,)
    args[0].append(5)
    args.append([7])
    print(repr((packed, len(packed), packed[0] is args[0])))


def wide(args: list[int]) -> None:
    packed = (*args,)
    print(repr((len(packed), packed[0], packed[-1], bool(packed))))
    try:
        print(packed[len(packed)])
    except IndexError as exc:
        print(str(exc))


items = [3, 4]
print(captures(items))
print(items)
print(captures([]))
call_prefix(mark, [3, 4])
call_prefix(mark, [])
call_prefix(Worker(40).add, [4])
tail_marker([])
tail_marker([1, 2])
midway([1, 2, 3])
midway([1, 3])
bad(7)
nested([[1], [2]])
wide(list(range(1025)))
shared = [4, 6]
print(repr((*shared, mutate(shared), *shared)))
