from collections import Counter
from dataclasses import dataclass, field
from typing import Self


@dataclass
class Totals:
    counts: Counter[str] = field(default_factory=Counter)


def check_typed_counts() -> None:
    a = Totals()
    b = Totals()
    print(a.counts["missing"])
    print(len(a.counts))
    print(a.counts.get("missing") is None)
    a.counts["stored"] += 0
    print(a.counts == b.counts)
    a.counts["one"] += 1
    a.counts["big"] += (1 << 90)
    a.counts.update(a.counts)
    b.counts.update(a.counts)
    print(a.counts == b.counts)
    print(sum(a.counts.values(), -3))
    print(len(set(a.counts.keys()).union(b.counts.keys())))
    for name, count in b.counts.items():
        print(name)
        print(count)


# Ruff's four fields and these five methods are unchanged from
# python/ruff-ecosystem/ruff_ecosystem/check.py at 3e4fffb05a2b.
@dataclass(frozen=True)
class RuleChanges:
    added_violations: Counter = field(default_factory=Counter)
    removed_violations: Counter = field(default_factory=Counter)
    added_fixes: Counter = field(default_factory=Counter)
    removed_fixes: Counter = field(default_factory=Counter)

    def update(self, other: Self) -> Self:
        self.added_violations.update(other.added_violations)
        self.removed_violations.update(other.removed_violations)
        self.added_fixes.update(other.added_fixes)
        self.removed_fixes.update(other.removed_fixes)
        return self

    def total_added_violations(self) -> int:
        return sum(self.added_violations.values())

    def total_removed_violations(self) -> int:
        return sum(self.removed_violations.values())

    def total_added_fixes(self) -> int:
        return sum(self.added_fixes.values())

    def total_removed_fixes(self) -> int:
        return sum(self.removed_fixes.values())


def plain_alias(c: dict[str, int]) -> None:
    print(c["missing"])
    print(c.get("missing") is None)
    print("missing" in c)
    print(repr([c[key] for key in "ab"]))
    print(repr([c[key] for key in "aéβ"]))
    del c["missing"]


def touch(source: list[str], c: Counter[str]) -> int:
    source.append("second")
    print(len(c))
    return 2


class Producer:
    def __init__(self) -> None:
        pass

    def values(self) -> list[int]:
        return [4, 7]


def bad(value: int) -> int:
    if value == 2:
        raise ValueError("after one item")
    return value


def main() -> None:
    check_typed_counts()
    big = 1267650600228229401496703205377
    # Missing counts are zero on both sides, even for disjoint key types.
    negative = Counter(a=-big)
    positive = Counter({1: big})
    assert negative < Counter() < positive
    assert not (positive < negative or negative > positive)
    assert Counter({1: 0}) == Counter(a=0)
    assert not (negative < negative or positive > positive)
    assert not (Counter(a=1) <= Counter(b=1) or Counter(a=1) >= Counter(b=1))
    assert len(negative) == 1 and len(positive) == 1
    was = Producer()
    print(sum((was := Producer()).values()))
    tied = Counter({"first": 0, "second": -1, "third": 0})
    del tied["first"]
    tied["first"] = 0
    assert repr(tied) == "Counter({'third': 0, 'first': 0, 'second': -1})"
    c = Counter(a=big, z=0)
    plain_alias(Counter(a=1))
    plain_alias(c)
    print(len(c))
    print(c)
    c.update(c)
    c.update({"a": -big, "extra": 0}, z=4)
    c.update("aba")
    c.update()
    print(c)
    print(c.total())
    source = ["first"]
    c.update(source, from_keyword=touch(source, c))
    print(c)
    try:
        c.update(["untouched"], later=bad(2))
    except ValueError as error:
        print(str(error))
        print("untouched" in c)
    integers: Counter[int] = Counter()
    integers[big] += big
    try:
        integers.update(bad(n) for n in [1, 2, 3])
    except ValueError as error:
        print(str(error))
        print(integers)
    print(integers[123])
    print(len(integers))
    print(integers.get(123, -5))
    print(Counter[int]([big, big, -1]))
    print(Counter(range(3)))
    print(Counter({}))
    print(Counter(("a", "a", "b")))
    print(Counter("αββ"))
    print(Counter(a=1, z=0) == Counter("a"))
    print(Counter(a=-1) != Counter())
    print(Counter(a=1, z=0))
    print(Counter())
    first = RuleChanges()
    second = RuleChanges()
    second.added_violations["F401"] += big
    second.removed_violations["F401"] += 3
    second.added_fixes.update(["I001", "I001"])
    second.removed_fixes.update(I001=-1)
    aggregate = first.update(second)
    aggregate.update(second)
    print(aggregate.total_added_violations())
    print(aggregate.total_removed_violations())
    print(aggregate.total_added_fixes())
    print(aggregate.total_removed_fixes())
    left = RuleChanges()
    left.removed_fixes["zero"] = 0
    print(left == RuleChanges())
    print(left)
    print(second.added_violations["F401"])
    print(len(RuleChanges().added_violations))
    displayed_changes_per_rule = Counter()
    for rule_code in ["F401", "I001", "F401", "F401"]:
        if displayed_changes_per_rule[rule_code] > 1:
            continue
        displayed_changes_per_rule[rule_code] += 1
        print(rule_code)
        print(displayed_changes_per_rule[rule_code])


main()
