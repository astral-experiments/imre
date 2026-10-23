from collections import defaultdict
from pathlib import Path


def replace(parts: list[str]) -> int:
    parts[0] = "changed"
    return 1000000000000000000000000


def fill(table: dict[tuple[str, int], str], spelling: str) -> str:
    table[(spelling, 1000000000000000000000000)] = "inserted"
    return "fallback"


def fail(parts: list[str]) -> int:
    parts.pop()
    raise ValueError("key field failed")


def name(number: int) -> str:
    return f"é-{number}-owner-replaced-by-the-next-key-component"


def option(present: bool) -> str | None:
    return "" if present else None


def main() -> None:
    table: dict[tuple[str, int], str] = {}
    parts = [name(len(table))]
    print(table.get((parts[0], replace(parts)), fill(table, name(0))))
    print((name(0), 1000000000000000000000000) in table)
    try:
        print(table.get((parts[0], fail(parts)), "missing"))
    except ValueError as error:
        print(str(error))
    print(len(parts))
    paths: set[tuple[Path, str | None]] = {(Path("a/b"), option(False))}
    print((Path("a/./b"), option(False)) in paths)
    print((Path("a/b"), option(True)) not in paths)
    buckets: defaultdict[tuple[str, int], list[str]] = defaultdict(list)
    print(buckets.get(("temporary", 3)) is None)
    print(len(buckets))
    buckets[("persistent", 4)].append("first")
    print(buckets[("persistent", 4)][0])


main()
