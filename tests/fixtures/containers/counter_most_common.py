from collections import Counter
from dataclasses import dataclass
from http import HTTPStatus


@dataclass(frozen=True)
class Key:
    value: int


def take(counts: Counter[str], n: int | None) -> list[tuple[str, int]]:
    return counts.most_common(n=n)


def take_status(counts: Counter[str], n: HTTPStatus | None) -> list[tuple[str, int]]:
    return counts.most_common(n)


def take_flag(counts: Counter[str], n: bool | None) -> list[tuple[str, int]]:
    return Counter.most_common(n=n, self=counts)


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
print(counts.most_common(HTTPStatus.CONTINUE))
print(take_status(counts, HTTPStatus.OK))
print(take_status(counts, None))
print(take_flag(counts, True))
print(take_flag(counts, False))
print(take_flag(counts, None))
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


# Equal counts never compare the keys. These records have no ordering methods.
keys: Counter[Key] = Counter()
keys[Key(7)] = 2
keys[Key(3)] = 1
keys[Key(4)] = 2
print(keys.most_common(2))
print(keys.most_common())

# Exercise every cutoff across tied, negative, zero and positive counts.
for first in range(-2, 3):
    for second in range(-2, 3):
        for third in range(-2, 3):
            integers = Counter({7: first, 3: second, 4: third})
            for n in range(-1, 5):
                print(integers.most_common(n))
