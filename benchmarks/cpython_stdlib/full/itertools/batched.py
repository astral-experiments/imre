"""Compare batch ownership, pull errors and argument conversion with pinned CPython."""

import csv
import sys
from io import StringIO
from itertools import batched, count, cycle, islice, tee


class Index:
    def __init__(self, size: int):
        self.size = size

    def __index__(self) -> int:
        print(f'index {self.size}')
        return self.size


class BadIndex:
    def __index__(self) -> str:
        print('bad index')
        return 'bad'


class Strict:
    def __bool__(self) -> bool:
        print('strict')
        raise RuntimeError('strict failed')


def input_expression() -> str:
    print('input expression')
    return 'abcd'


def size_expression() -> int:
    print('size expression')
    return 2


def growing_file(path: str, strict: bool) -> None:
    with open(path, 'w', encoding='utf-8') as seed:
        seed.write('one\n')
    with open(path, encoding='utf-8') as reader:
        cursor = batched(reader, 2, strict=strict)
        try:
            print(next(cursor))
        except ValueError as exc:
            print(str(exc))
        with open(path, 'ab') as writer:
            writer.write(b'two\n')
        try:
            print(next(cursor))
        except StopIteration:
            print('still exhausted')
        # An empty pull now exhausts even a non-strict batch permanently.
        try:
            next(cursor)
        except StopIteration:
            print('exhausted')
        with open(path, 'ab') as writer:
            writer.write(b'three\n')
        try:
            next(cursor)
        except StopIteration:
            print('still exhausted')
        print(repr(reader.readline()))


def stream_cases() -> None:
    text = StringIO('skip\r\na\r\nb\nc', newline='')
    grouped = batched(text, 2)
    print(repr(text.readline()))
    print(repr(next(grouped)))
    print(repr(next(grouped)))
    text.seek(0)
    print(repr(next(grouped)))  # A short, non-strict batch leaves the source live.
    text.close()
    try:
        next(grouped)
    except ValueError as exc:
        print(str(exc))
    try:
        next(grouped)
    except StopIteration:
        print('closed StringIO exhausted')
    try:
        batched(text, 1)
    except ValueError as exc:
        print(str(exc))

    source = ['a,b', 'c,d', 'e,f']
    rows = csv.reader(source)
    batches = batched(rows, 2)
    print(next(rows))
    source.append('g,h')
    print(repr(next(batches)))
    print(rows.line_num)
    print(repr(next(batches)))
    try:
        next(batches)
    except StopIteration:
        print('CSV exhausted')
    bad = csv.reader(['first', '"unterminated'], strict=True)
    partial = batched(bad, 2)
    try:
        next(partial)
    except csv.Error as exc:
        print(str(exc))
        print(bad.line_num)
    try:
        next(partial)
    except StopIteration:
        print('CSV error exhausted')

    # Use the same short checked-in text for the CPython and native processes.
    with open('benchmarks/cpython_stdlib/full/itertools/upstream/batched.doctest', encoding='utf-8') as stream:
        groups = batched(stream, 2)
        print(repr(stream.readline()))
        print(repr(next(groups)))
    try:
        next(groups)
    except ValueError as exc:
        print(str(exc))
    try:
        next(groups)
    except StopIteration:
        print('closed input exhausted')
    try:
        batched(stream, 0)
    except ValueError as exc:
        print(str(exc))
    try:
        batched(stream, 1)
    except ValueError as exc:
        print(str(exc))
    print(len(tee(stream, 0)))
    try:
        tee(stream, 2)
    except ValueError as exc:
        print(str(exc))
    try:
        cycle(stream)
    except ValueError as exc:
        print(str(exc))
    try:
        islice(stream, 0)
    except ValueError as exc:
        print(str(exc))


def main() -> None:
    cursor = count(7)
    # The supported native targets use 64-bit Py_ssize_t. This tuple cannot fit.
    oversized = batched(cursor, (1 << 63) - 1)
    for _ in range(2):
        try:
            next(oversized)
        except MemoryError:
            print(next(cursor))
    source = [2, 4, 6]
    batches = batched(source, 2)
    alias = iter(batches)
    first = next(alias)
    print(first)
    print(first[0])
    print(first[-1])
    try:
        first[9]
    except IndexError as exc:
        print(str(exc))
    source.append(8)
    print(next(batches))
    try:
        next(alias)
    except StopIteration:
        print('empty')
    source.append(10)
    try:
        next(batches)
    except StopIteration:
        print('still empty')
    short = batched('ABC', 2, strict=True)
    print(next(short))
    try:
        next(short)
    except ValueError as exc:
        print(str(exc))
    try:
        next(short)
    except StopIteration:
        print('incomplete batch exhausted')
    mapping = {'a': 7, 'b': 8}
    partial = batched(mapping, 1)
    print(next(partial))
    mapping['c'] = 9
    try:
        next(partial)
    except RuntimeError as exc:
        print(str(exc))
    try:
        next(partial)
    except StopIteration:
        print('input error exhausted')
    saved = list(batched('abcdefg', 2))
    print(repr(saved))
    print(saved[0] == ('a', 'b'))
    print(saved[3] == ('g',))
    print(saved[0] != ('b', 'a'))
    print(saved[3] != saved[0])
    print(next(batched(n=Index(2), iterable=input_expression())))
    for n in (0, 1 << 100, -(1 << 100)):
        try:
            batched(None, Index(n), strict=Strict())
        except OverflowError as exc:
            print(str(exc))
        except RuntimeError as exc:
            print(str(exc))
    try:
        batched('abc', BadIndex(), strict=Strict())
    except TypeError as exc:
        print(str(exc))
    try:
        batched('abc', 'hello', strict=Strict())
    except TypeError as exc:
        print(str(exc))
    try:
        batched(input_expression())
    except TypeError as exc:
        print(str(exc))
    try:
        batched(input_expression(), 2, size_expression())
    except TypeError as exc:
        print(str(exc))
    try:
        batched(input_expression(), 2, stric=True)
    except TypeError as exc:
        print(str(exc))
    try:
        batched(None, 0)
    except ValueError as exc:
        print(str(exc))
    try:
        batched(None, 1)
    except TypeError as exc:
        print(str(exc))


main()
stream_cases()
if len(sys.argv) > 1:
    growing_file(sys.argv[1], False)
    growing_file(sys.argv[1], True)
