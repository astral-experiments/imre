from collections import Counter
from dataclasses import dataclass, field


@dataclass
class Totals:
    counts: Counter[str] = field(default_factory=Counter)


def main() -> None:
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


main()
