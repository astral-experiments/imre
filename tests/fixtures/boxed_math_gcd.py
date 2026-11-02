import math
from math import gcd


class Integral(int):
    def __index__(self):
        raise AssertionError("the native int value takes precedence")


class Indexed:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        visited.append(self.value)
        return self.value


visited = []
assert gcd() == 0 and gcd(0, 0) == 0
assert gcd(False, True) == 1 and type(gcd(True)) is int
assert gcd(Integral(-27)) == 27
big = 1 << 180
assert math.gcd(0, -9 * big, 6 * big) == 3 * big
assert gcd(Indexed(-35), 4, Indexed(11)) == 1
assert visited == [-35, 11]
for args in [(1, 2.0), (1, Indexed("bad"))]:
    try:
        gcd(*args)
    except TypeError:
        pass
    else:
        raise AssertionError(args)
try:
    gcd(a=4, b=6)
except TypeError:
    pass
else:
    raise AssertionError("accepted keywords")
