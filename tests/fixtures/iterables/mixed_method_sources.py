from collections.abc import Iterator


class Source:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values

    def rows(self, marker: int):
        print("list " + str(marker))
        return self.values


class Lazy(Source):
    def rows(self, marker: int) -> Iterator[int]:
        print("generator " + str(marker))
        yield from self.values


def mark() -> int:
    print("argument")
    return 1


def observe(source: Source) -> None:
    empty: list[int] = []
    for pair in zip(empty, source.rows(mark())):
        print(pair)
    try:
        print(list(reversed(source.rows(mark()))))
    except TypeError as error:
        print(str(error))
    for value in source.rows(mark()):
        print(value)
        break


def extend(destination: list[int], source: Source) -> None:
    destination.extend(source.rows(mark()))
    print(destination)


class SortKey:
    destination: list[int]
    source: Source

    def __init__(self, destination: list[int], source: Source) -> None:
        self.destination = destination
        self.source = source

    def key(self, value: int) -> int:
        self.destination.extend(self.source.rows(0))
        return value


def sort_empty(source: Source) -> None:
    values = [2, 1]
    try:
        values.sort(key=SortKey(values, source).key)
        print("sorted")
    except ValueError:
        print("changed")
    print(values)


original = [3, 4]
observe(Source(original))
observe(Lazy(original))
extend(original, Source(original))
extend(original, Lazy([5]))
sort_empty(Source([]))
sort_empty(Lazy([]))
