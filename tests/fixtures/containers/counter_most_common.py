from collections import Counter
from dataclasses import dataclass


def take(counts: Counter[str], n: int | None) -> list[tuple[str, int]]:
    return counts.most_common(n=n)


def limit() -> int:
    print("limit")
    return 3


def receiver(counts: Counter[str]) -> Counter[str]:
    print("receiver")
    return counts


counts = Counter("abracadabra")
print(counts.most_common())
print(take(counts, None))
print(take(counts, -1))
print(take(counts, 0))
print(take(counts, 1))
print(take(counts, 3))
print(counts.most_common(True))
print(counts.most_common(False))
print(Counter.most_common(counts, 2))
print(Counter.most_common(n=limit(), self=receiver(counts)))
big = 10000000000000000000000000000000000000000
counts["zero"] = 0
counts["negative"] = -big
counts["large"] = big
saved = take(counts, big)
print(saved)
print(take(counts, -big))
counts["a"] = -10
print(saved)
del counts["b"]
counts["b"] = 1
print(counts.most_common())


@dataclass(frozen=True)
class Key:
    value: int


# Equal counts never compare the keys. These records have no ordering methods.
keys = Counter([Key(7), Key(3), Key(4), Key(7), Key(4)])
print(keys.most_common(2))
print(keys.most_common())
