import math
from math import isnan


class FloatOnly:
    def __float__(self):
        return float("nan")


class IndexOnly:
    def __index__(self):
        return 7


assert isnan is math.isnan
assert isnan(float("nan")) is True
assert isnan(1.25) is False
assert isnan(float("inf")) is False
assert isnan(FloatOnly()) is True
assert isnan(IndexOnly()) is False
try:
    isnan("nan")
except TypeError as exc:
    assert str(exc) == "must be real number, not str"
else:
    raise AssertionError("isnan accepted a string")
