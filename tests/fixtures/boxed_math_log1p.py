from math import copysign, log1p


class Estimate:
    def __float__(self):
        return 1e-30


assert log1p(Estimate()) == 1e-30
assert log1p(-1e-30) == -1e-30
assert copysign(1.0, log1p(-0.0)) == -1.0
try:
    log1p(-1)
except ValueError as exc:
    assert str(exc) == "expected argument value > -1, got -1.0"
else:
    raise AssertionError("accepted the logarithm of zero")

print("log1p ok")
