from collections import Counter, defaultdict


def install(values: dict[str, int], seen: list[int]) -> int:
    seen.append(1)
    values["late"] = 42
    return 99


def main() -> None:
    values = {"b": 2, "a": 1}
    print(values.setdefault("a", 9))
    print(list(values.items()))
    print(dict.setdefault(values, "c", 3))
    print(list(values.items()))
    seen: list[int] = []
    print(values.setdefault("a", install(values, seen)))
    print(values.setdefault("late", install(values, seen)))
    print(seen)
    print(list(values.items()))

    mutable: dict[str, list[int]] = {}
    saved: list[int] = []
    other: list[int] = []
    print(mutable.setdefault("k", saved) is saved)
    mutable.setdefault("k", other).append(3)
    print(saved)
    print(other)
    optional: dict[str, int | None] = {}
    print(optional.setdefault("none") is None)
    print(optional.setdefault("none", 19) is None)
    present = optional.setdefault("value", 19)
    if present is None:
        print("unexpected None")
    else:
        print(present)
    print(optional)

    tuples: dict[tuple[int, str], str] = {}
    print(tuples.setdefault((7, "k"), "first"))
    print(tuples.setdefault((7, "k"), "second"))
    print(tuples)
    nested: dict[tuple[tuple[int, str], int], int] = {}
    for n in [1, 1, 2]:
        print(nested.setdefault(((n, "k"), 0), len(nested)))
    print(nested)
    counts = Counter("aba")
    print(counts.setdefault("a", 5))
    print(Counter.setdefault(counts, "e", 5))
    print(counts["e"])
    groups: defaultdict[str, list[int]] = defaultdict(list)
    print(groups.setdefault("new", saved) is saved)
    print(groups["new"])
    print(groups["factory"])


def nested_key(value: int) -> tuple[tuple[str, int], bytes]:
    return (("key", value), b"suffix")


def nested_tuple_keys() -> None:
    table: dict[tuple[tuple[str, int], bytes], list[int]] = {}
    stored = [1]
    other = [2]
    large = 10**48
    print(table.setdefault(nested_key(large), stored) is stored)
    print(table.setdefault(nested_key(large), other) is stored)
    print(table.setdefault(nested_key(large + 1), other) is other)
    print(len(table))
    print(stored)
    print(other)


main()
nested_tuple_keys()
