def current():
    calls = []

    class Left(int):
        def __rshift__(self, other):
            calls.append("left")
            return NotImplemented

        __lshift__ = __rshift__

    class Right(int):
        def __rrshift__(self, other):
            calls.append("right")
            return NotImplemented

        __rlshift__ = __rrshift__

    class Handled(Right):
        def __rrshift__(self, other):
            return "reflected"

        __rlshift__ = __rrshift__

    class Sibling(int):
        pass

    for op in (lambda a, b: a >> b, lambda a, b: a << b):
        for count in (-1, 2):
            calls.clear()
            try:
                op(Left(7), Right(count))
            except TypeError:
                pass
            else:
                raise AssertionError("both integer shift slots declined")
            assert calls == ["left", "right"]

        assert op(Right(7), Handled(-1)) == "reflected"
        calls.clear()
        assert op(Sibling(0), Right(2)) == 0
        assert calls == []
        try:
            op(7, -1)
        except ValueError as error:
            assert str(error) == "negative shift count"
        else:
            raise AssertionError("integer slot accepted a negative shift count")
