"""Exercise the argument display in threading_helper.run_concurrently."""
from annotationlib import Format, ForwardRef, get_annotations
from itertools import islice
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


def shared_capture(shared: list[int]) -> tuple[int, ...]:
    return (*shared, mutate(shared), *shared)


def arguments(args: list[int]) -> tuple[int, ...]:
    return (*args,)


def cursor_capture(args: list[int]) -> None:
    cursor = islice(args, len(args))
    print(next(cursor, -1))
    print(repr((0, *cursor, mark(10))))
    print(next(cursor, -1))


def call_prefix(func: Callable[[int], int], args: tuple[int, ...]) -> None:
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


def pointer_fields() -> None:
    classes = [bool, int, str]
    packed_classes = (*classes,)
    classes.clear()
    for i in range(len(packed_classes)):
        print(issubclass(packed_classes[i], int))

    refs = [ForwardRef("First", owner=int), ForwardRef("Second", owner=int)]
    first = refs[0]
    packed_refs = (*refs,)
    refs.clear()
    print(packed_refs[0] is first)
    print(packed_refs[-1].__forward_arg__)
    print(packed_refs[-1].__owner__ is int)


def annotation_fields() -> None:
    def declared(value: int) -> missing: ...

    annotations = get_annotations(declared, format=Format.FORWARDREF)
    values = [annotations["value"], annotations["return"]]
    packed = (*values,)
    values.clear()
    del annotations["value"]
    del annotations["return"]
    print(packed[0] is int)
    reference = packed[-1]
    print(isinstance(reference, ForwardRef))
    print(reference.__forward_arg__)
    print(reference.__owner__ is declared)

items = [3, 4]
print(captures(items))
print(items)
print(captures([]))
call_prefix(mark, arguments([3, 4]))
call_prefix(mark, ())
call_prefix(Worker(40).add, (4,))
cursor_capture([1, 2, 3])
tail_marker([])
tail_marker([1, 2])
midway([1, 2, 3])
midway([1, 3])
bad(7)
nested([[1], [2]])
wide(list(range(1025)))
shared = [4, 6]
print(repr(shared_capture(shared)))
pointer_fields()
annotation_fields()
