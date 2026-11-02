from datetime import timedelta as Delta


class SubDelta(Delta):
    pass


class Factor(int):
    def __mul__(self, other):
        if isinstance(other, Delta):
            return "left integer slot"
        assert type(other) is int
        return other + 1

    def __rmul__(self, other):
        raise AssertionError("timedelta has the first slot")


def current():
    tick = Delta(microseconds=1)
    assert tick * 3 == 3 * tick == Delta(microseconds=3)
    scaled = tick
    scaled *= -2
    assert scaled == Delta(microseconds=-2) and tick == Delta(microseconds=1)
    assert Delta.__mul__.__doc__ == "Return self*value."
    assert Delta.__rmul__.__doc__ == "Return value*self."
    assert Delta.__mul__.__text_signature__ == Delta.__rmul__.__text_signature__ == "($self, value, /)"
    assert 0 * Delta.max == Delta.max * False == Delta()
    assert -3 * tick == tick * -3 == Delta(microseconds=-3)
    assert (1 << 76) * Delta() == Delta()
    assert tick * (1 << 65) == Delta(microseconds=1 << 65)
    assert Delta.__mul__(tick, True) == tick
    assert tick.__rmul__(7) == 7 * tick
    assert tick.__mul__([]) is NotImplemented
    assert tick * Factor(2) == Delta(microseconds=2)
    assert Factor(2) * tick == "left integer slot"
    assert type(SubDelta(microseconds=1) * 2) is Delta

    class Parts:
        def __divmod__(self, unit):
            assert unit == 1000000
            return 3, 4

    class CustomProduct(int):
        def __mul__(self, micros):
            return Parts()

    assert tick * CustomProduct(1) == Delta(seconds=3, microseconds=4)

    class OversizeParts(Parts):
        def __divmod__(self, unit):
            return 0, 1 << 31

    class OversizeProduct(int):
        def __mul__(self, micros):
            return OversizeParts()

    try:
        tick * OversizeProduct(1)
    except OverflowError as exc:
        assert str(exc) == "Python int too large to convert to C int"
    else:
        raise AssertionError("oversize custom remainder")

    class SubclassOverride(Delta):
        def __rmul__(self, other):
            return "narrower type"

    assert SubclassOverride() * 2 == Delta()
    assert 2 * SubclassOverride() == "narrower type"
    assert tick * SubclassOverride() == "narrower type"
    try:
        Delta.max * 2
    except OverflowError as exc:
        print(str(exc))
    else:
        raise AssertionError("unbounded timedelta")

    class Reflected:
        def __rmul__(self, left):
            assert left is tick
            return "fallback"

    assert tick * Reflected() == "fallback"
    for name in ("__mul__", "__rmul__"):
        try:
            getattr(tick, name)(n=1)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("accepted keyword argument")
