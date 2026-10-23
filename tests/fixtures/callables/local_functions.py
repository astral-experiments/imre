from dataclasses import dataclass
from pathlib import Path


def lookups(table: dict[int, str]) -> None:
    def lookup(key: int, *, default: str = "?") -> str:
        return table.get(key, default)

    def earlier_helper(table: int) -> str:
        return lookup(table)

    print(earlier_helper(7))
    alias = table
    alias[7] = "mutated"
    print(lookup(7))
    table = {8: "new"}
    print(",".join([earlier_helper(index) for index in [7, 8]]))
    for key in [7, 8]:
        print(lookup(key, default="absent"))


def late(offset: int) -> int:
    def add(value: int) -> int:
        return offset + value

    return add(offset := offset + 1)


@dataclass
class Row:
    path: Path
    value: int


def same(left: list[Row], right: list[Row]) -> bool:
    def key(row: Row) -> tuple[str, int]:
        return (str(row.path), row.value)

    return sorted(map(key, left)) == sorted(map(key, right))


def trace(left: list[int], right: list[int]) -> bool:
    def emit(i: int) -> tuple[int]:
        print(i)
        return (i,)

    return sorted(map(emit, left)) != sorted(map(emit, right))


def main() -> None:
    lookups({7: "old"})
    print(late(3))
    a = Row(Path("p/./x"), 4)
    b = Row(Path("p/x"), 4)
    c = Row(Path("p/../x"), 4)
    print(same([], []))
    print(same([a], []))
    print(same([a], [b]))
    print(same([a], [c]))
    print(same([a, c, a], [c, b, b]))
    print(trace([1, 2], [3]))


main()
