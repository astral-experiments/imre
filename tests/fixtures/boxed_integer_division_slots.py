def current():
    class Left(int):
        def __floordiv__(self, other):
            return NotImplemented

        __mod__ = __floordiv__

    class Right(int):
        def __rfloordiv__(self, other):
            return NotImplemented

        __rmod__ = __rfloordiv__

    class Sibling(int):
        pass

    class Handled(Sibling):
        def __rfloordiv__(self, other):
            return 42

        __rmod__ = __rfloordiv__

    class Late:
        def __rfloordiv__(self, other):
            return 43

        __rmod__ = __rfloordiv__

    class Forward(int, Late):
        def __floordiv__(self, other):
            return 44

        __mod__ = __floordiv__

    class Reintroduced(Forward):
        __rfloordiv__ = Late.__rfloordiv__
        __rmod__ = Late.__rmod__

    for op, result in ((lambda a, b: a // b, -3), (lambda a, b: a % b, 2)):
        try:
            op(Left(-7), Right(3))
        except TypeError:
            pass
        else:
            raise AssertionError("both integer slots declined")
        assert op(Left(-7), Sibling(3)) == result
        assert op(Sibling(-7), Right(3)) == result
        assert op(Sibling(-7), Handled(3)) == 42
        assert op(Forward(-7), Reintroduced(3)) == 43
