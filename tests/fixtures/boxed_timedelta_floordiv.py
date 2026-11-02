from datetime import timedelta as Delta


class Duration(Delta):
    def __rfloordiv__(self, left):
        return "reflected duration"


class Parts:
    def __divmod__(self, unit):
        assert unit == 1_000_000
        return 2, 5


class Divisor(int):
    def __rfloordiv__(self, micros):
        assert type(micros) is int and micros == 7
        return Parts()


def current():
    tick = Delta(microseconds=1)
    seven = Delta(microseconds=7)
    neg_seven = Delta(microseconds=-7)
    assert seven // 3 == Delta(microseconds=2)
    assert neg_seven // 3 == seven // -3 == Delta(microseconds=-3)
    assert neg_seven // -3 == Delta(microseconds=2)
    assert seven // True == seven
    assert neg_seven // (1 << 500) == Delta(microseconds=-1)
    assert seven // Divisor(3) == Delta(seconds=2, microseconds=5)
    assert type(Duration(microseconds=7) // 3) is Delta

    assert type(seven // tick) is int
    assert Delta(microseconds=9007199254740993) // tick == 9007199254740993
    assert neg_seven // Delta(microseconds=3) == -3
    assert Delta.__rfloordiv__(Delta(microseconds=3), neg_seven) == -3
    assert Duration(microseconds=7) // tick == 7
    assert tick // Duration(microseconds=7) == "reflected duration"
    assert tick.__floordiv__(2.0) is NotImplemented
    assert tick.__rfloordiv__(2) is NotImplemented
    assert Delta.__floordiv__.__doc__ == "Return self//value."
    assert Delta.__rfloordiv__.__text_signature__ == "($self, value, /)"
    for zero in (0, False, Delta()):
        try:
            seven // zero
        except ZeroDivisionError as exc:
            assert str(exc) == "division by zero"
        else:
            raise AssertionError("accepted a zero divisor")
    try:
        Delta.max // -1
    except OverflowError:
        pass
    else:
        raise AssertionError("accepted an out-of-range timedelta")
