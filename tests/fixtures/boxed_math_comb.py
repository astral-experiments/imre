from importlib import import_module
from math import comb


def current():
    assert import_module("math").comb is comb
    assert comb(67, 33) == 14226520737620288370
    assert comb(68, 34) == 28453041475240576740
    assert comb(2048, 1024) == 2 * comb(2047, 1023)
    assert comb(16385, 1024) == comb(16384, 1023) + comb(16384, 1024)
    n = 2**1000
    assert comb(n, n - 2) == n * (n - 1) // 2
    assert comb(0, n) == 0
    assert type(comb(True, False)) is int

    events = []

    class Index:
        def __init__(self, value):
            self.value = value

        def __index__(self):
            events.append(self.value)
            return self.value

    assert comb(Index(8), Index(2)) == 28
    assert events == [8, 2]
    try:
        comb(Index(-1), Index("bad"))
    except TypeError:
        pass
    else:
        raise AssertionError("k must be indexed before checking the sign of n")
    assert events == [8, 2, -1, "bad"]
    print("comb ok")


if __name__ == "__main__":
    current()
