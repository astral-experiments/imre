from datetime import timedelta as Delta


class Ratio(float):
    def as_integer_ratio(self):
        return self.pair

    def __float__(self):
        raise AssertionError("converted subclass")


def current():
    tick = Delta(microseconds=1)
    for micros, rounded in ((-5, -2), (-3, -2), (-1, 0), (1, 0), (3, 2), (5, 2)):
        t = Delta(microseconds=micros)
        expected = Delta(microseconds=rounded)
        assert t * 0.5 == 0.5 * t == t / 2.0 == t / 2 == expected
    assert Delta(seconds=1) * 0.6112295 == Delta(microseconds=611229)
    assert Delta(seconds=1) / (1 / 0.6112295) == Delta(microseconds=611229)
    assert Delta.max * 1.0 == Delta.max / 1.0 == Delta.max
    assert Delta(microseconds=3) / Delta(microseconds=2) == 1.5
    assert Delta(microseconds=2**53 + 1) / Delta(microseconds=3) == 3002399751580331.0
    assert Delta.max * 5e-324 == Delta()
    custom = Ratio(11)
    custom.pair = (1, 2)
    assert tick * custom == custom * tick == Delta()
    assert tick / custom == Delta(microseconds=2)
    class Numerator(int):
        def __rmul__(self, micros):
            assert type(micros) is int
            return 2 * micros + 1

    three = Delta(microseconds=3)
    custom.pair = (Numerator(100), 2)
    assert three * custom == Delta(microseconds=4)
    custom.pair = (2, Numerator(100))
    assert three / custom == Delta(microseconds=4)
    assert Delta.__truediv__(tick, "3") is NotImplemented
    assert Delta.__rtruediv__(tick, 3) is NotImplemented
    assert Delta.__rtruediv__(tick, Delta(microseconds=7)) == 7.0
    for value, kind in ((float("nan"), ValueError), (float("inf"), OverflowError)):
        for op in (tick.__mul__, tick.__truediv__):
            try:
                op(value)
            except kind as exc:
                print(type(exc).__name__, str(exc))
            else:
                raise AssertionError("bad float accepted")
    for ratio, kind in ((1 << 1000, TypeError), ((), ValueError), ((1,), ValueError), ((1, 2, 3), ValueError)):
        custom.pair = ratio
        for op in (tick.__mul__, tick.__truediv__):
            try:
                op(custom)
            except kind as exc:
                print(type(exc).__name__, str(exc))
            else:
                raise AssertionError("bad ratio accepted")
    for zero in (0, 0.0, -0.0, Delta()):
        try:
            tick / zero
        except ZeroDivisionError as exc:
            print(type(exc).__name__, str(exc))
        else:
            raise AssertionError("zero divisor accepted")
