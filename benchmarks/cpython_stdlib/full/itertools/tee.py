"""Supplemental tee checks; keep sources live and independent of fork positions."""
from itertools import islice, tee


def main() -> None:
    print(len(tee(42, 0)))
    items = [1, 2]
    a, b, c = tee(items, 3)
    print(next(a))
    items[0] = 8
    items.append(9)
    print(next(b))
    print(next(c))
    print(next(a))
    d, e = tee(b)
    print(next(d))
    print(next(b))
    print(next(e))
    del b, c
    print(repr(list(a)))
    print(repr(list(d)))
    print(repr(list(e)))
    fast, slow = tee(range(1 << 16))
    print(sum(islice(fast, 65535)))
    print(next(slow))
    del slow
    print(next(fast))
    print(next(fast, -1))
    mapping = {"a": 3, "b": 1}
    x, y = tee(mapping)
    print(next(x))
    mapping["late"] = 2
    try:
        next(x)
    except RuntimeError:
        print("leader saw mutation")
    print(next(y))
    try:
        next(y)
    except RuntimeError:
        print("lagger saw mutation")


main()
