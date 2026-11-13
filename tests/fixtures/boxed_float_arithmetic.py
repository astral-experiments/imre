def add(a,b): return a+b
def sub(a,b): return a-b
def mul(a,b): return a*b
def div(a,b): return a/b
def floordiv(a,b): return a//b
def mod(a,b): return a%b
def current():
    for value in (-(1 << 130), True, False, -0.0, float('-inf')):
        magnitude = abs(value)
        print(type(magnitude).__name__, repr(magnitude))
    for call in (lambda: abs(), lambda: abs(1, 2), lambda: abs(x=2)):
        try:
            call()
        except TypeError as error:
            print(error.args)
        else:
            raise AssertionError("invalid abs call accepted")
    for name,func in (('+',add), ('-',sub), ('*',mul), ('/',div), ('//',floordiv), ('%',mod)):
        for a,b in ((1.5,0.5),(1,0.5),(0.5,1),(-7.0,3.0),(7.0,-3.0),(0.0,-3.0),(-0.0,3.0),(1.0,0.0),(1e308,1e-308)):
            try: print(name,repr(func(a,b)))
            except Exception as error: print(name,type(error).__name__,str(error))

    try:
        add(4, "")
    except TypeError as error:
        print(str(error))
    value = 4
    try:
        value += ""
    except TypeError as error:
        print(str(error))

    for a,b in ((2.0, 3.0), (-2.0, 3.0), (-4.0, 0.5), (-0.0, 3.0), (0.0, -1.0), (1.0, float("nan")), (1e300, 2.0)):
        try: print("**", repr(a**b))
        except Exception as error: print("**", type(error).__name__, str(error))

    print((-2) ** -3, pow(base=2, exp=-4), pow(2, -3, -13))
    try:
        pow(6, -1, 18)
    except ValueError as error:
        print(str(error))

    for call in (lambda: pow(2, base=3), lambda: pow(2, 3, base=4),
                 lambda: pow(2, 3, modd=4), lambda: pow(2, 3.0, 5)):
        try:
            call()
        except TypeError as error:
            print(error.args)

    for operation, value in ((lambda x: -x, "x"), (lambda x: +x, []), (lambda x: ~x, 2.5)):
        try:
            operation(value)
        except TypeError as error:
            print(str(error))

    class Declines:
        def __neg__(self):
            return NotImplemented

    print(-Declines() is NotImplemented)

    class Named(float):
        def __float__(self):
            raise AssertionError("negation converted its receiver")

    class Override(Named):
        def __neg__(self):
            return ("override", super().__neg__())

    value = -Named(0.0)
    print(type(value).__name__, value.hex())
    print(-Override(2.5), float.__neg__(Override(-2.5)))

    class Quotient(float):
        def __rtruediv__(self, other):
            divide = super().__rtruediv__
            assert divide.__self__ is self
            return type(self)(divide(other))

    result = 1 / Quotient(8)
    assert type(result) is Quotient and result == 0.125
    assert float.__truediv__(result, object()) is NotImplemented

    class Left:
        def __truediv__(self, other):
            return NotImplemented

    class Right:
        def __rtruediv__(self, other):
            return NotImplemented

    left, right = Left(), Right()
    try:
        left / right
    except TypeError as error:
        assert str(error) == "unsupported operand type(s) for /: 'Left' and 'Right'"
    else:
        raise AssertionError("declined division succeeded")
    try:
        left /= right
    except TypeError as error:
        assert str(error) == "unsupported operand type(s) for /=: 'Left' and 'Right'"
    else:
        raise AssertionError("declined in-place division succeeded")

    class DeclinesDivision(float):
        def __truediv__(self, other):
            return NotImplemented

    try:
        DeclinesDivision(4) / DeclinesDivision(2)
    except TypeError:
        pass
    else:
        raise AssertionError("declined division retried the float payload")

    class Forward(float):
        def __truediv__(self, other):
            return "forward"

    class Inherits(Forward):
        pass

    class Restores(Forward):
        __truediv__ = float.__truediv__

    assert Forward(6) / Inherits(3) == "forward"
    assert Forward(6) / Restores(3) == 2.0

    from abc import ABCMeta

    class VirtualBase(int, metaclass=ABCMeta):
        def __truediv__(self, other):
            return "int forward"

    VirtualBase.register(float)
    assert VirtualBase(6) / 3.0 == "int forward"
