from collections import namedtuple

Pair = namedtuple("Pair", ("name", "count"))


def make(name: str, count: int) -> Pair:
    return Pair(name, count)


FIRST = make("imported", 7)
