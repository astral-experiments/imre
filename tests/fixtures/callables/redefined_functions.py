def score(n: int) -> int:
    if n < 0:
        return n * 3
    return n + 9


def f(n: int) -> int:
    return n - 2


old = f
print(f(4), old(4), old is f)


def f(n: int) -> int:
    return old(n) + 7


print(old(4), f(4), old is f, score(-2), score(2))
