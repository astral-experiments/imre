import math
from math import nextafter


class FloatArg:
    def __init__(self, value, events):
        self.value = value
        self.events = events

    def __float__(self):
        self.events.append(self.value)
        return self.value


class Steps:
    def __init__(self, events):
        self.events = events

    def __index__(self):
        self.events.append("steps")
        return 4


def current():
    assert math.nextafter is nextafter
    tiny = 5e-324
    assert math.copysign(1, nextafter(tiny, -0.0)) == 1
    assert math.copysign(1, nextafter(tiny, -0.0, steps=1)) == -1
    assert nextafter(-tiny, tiny, steps=2) == tiny
    assert nextafter(-math.inf, math.inf, steps=10**100) == math.inf
    events = []
    assert nextafter(FloatArg(0.0, events), FloatArg(1.0, events), steps=Steps(events)) == 4 * tiny
    assert events == [0.0, 1.0, "steps"]

    math.nextafter = 37
    try:
        assert math.nextafter == 37
        del math.nextafter
        assert not hasattr(math, "nextafter")

        def fallback(name):
            events.append(name)
            return 83

        math.__getattr__ = fallback
        try:
            assert math.nextafter == 83
            assert events == [0.0, 1.0, "steps", "nextafter"]
        finally:
            del math.__getattr__
    finally:
        math.nextafter = nextafter


if __name__ == "__main__":
    current()
