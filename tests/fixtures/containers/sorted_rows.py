from collections import defaultdict
from dataclasses import dataclass


@dataclass
class Row:
    name: str
    first: list[int]
    second: list[int]
    again: list[int]
    names: list[str]


def rows(
    left: defaultdict[str, list[int]],
    right: defaultdict[str, list[int]],
    names: defaultdict[str, list[str]],
    extra: defaultdict[str, list[int]],
) -> list[Row]:
    keys = sorted(left.keys() | right.keys() | names.keys() | extra.keys())
    return [Row(k, left[k], right[k], left[k], names[k]) for k in keys]


def replace(left: defaultdict[str, list[int]], key: str) -> list[int]:
    previous = left[key]
    left[key] = [777]
    return previous


def with_effects(
    left: defaultdict[str, list[int]], right: defaultdict[str, list[int]]
) -> list[Row]:
    keys = sorted(left.keys() | right.keys())
    return [Row(k, left[k], replace(left, k), left[k], []) for k in keys]


def main() -> None:
    left: defaultdict[str, list[int]] = defaultdict(list)
    right: defaultdict[str, list[int]] = defaultdict(list)
    labels: defaultdict[str, list[str]] = defaultdict(list)
    extra: defaultdict[str, list[int]] = defaultdict(list)
    print(len(rows(left, right, labels, extra)))
    extra["a\0z"].append(9)
    labels["é"].append("label")
    left["中"].append(6)
    first = rows(left, left, labels, extra)
    print(repr([r.name for r in first]))
    print(repr([k for k in left]))
    print(repr([k for k in labels]))
    print(repr([k for k in extra]))
    first[0].first.append(23)
    print(repr(first[0].second))
    print(repr(first[0].again))
    print(repr(left["a\0z"]))
    del left["a\0z"]
    print(repr(first[0].first))
    print(repr(left["a\0z"]))
    print(repr(first[1].first))
    print(repr(first[1].names))

    other = rows(left, right, labels, extra)
    other[0].first.append(12)
    print(repr(other[0].second))
    print(repr(right["a\0z"]))
    print(repr(other[0].again))
    effects = with_effects(left, right)
    print(repr(effects[0].first))
    print(repr(effects[0].second))
    print(repr(effects[0].again))
    print(repr(left["a\0z"]))


main()
