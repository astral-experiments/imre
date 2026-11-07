from math import perm


class DrawCount:
    def __index__(self):
        return 2


def current():
    n = 1 << 100
    assert perm(n, DrawCount()) == n * (n - 1)
    assert perm(3) == perm(3, None) == 6
    assert perm(3, n) == 0
    try:
        perm(-1, object())
    except TypeError:
        pass
    else:
        raise AssertionError("both index conversions must precede the domain check")
    print("perm ok")


if __name__ == "__main__":
    current()
