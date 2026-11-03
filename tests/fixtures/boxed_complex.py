def current():
    assert callable(complex) and type(complex) is type
    assert complex.__name__ == complex.__qualname__ == "complex" and complex.__module__ == "builtins"
    events = []
    saved = complex(1, 2)

    class Child(complex):
        pass

    assert saved.__complex__() is saved and complex.__complex__.__objclass__ is complex
    assert complex.__complex__.__doc__ == "Convert this value to exact type complex."
    assert complex.__complex__.__text_signature__ == "($self, /)"
    child = Child(saved)
    converted = complex.__complex__(child)
    assert type(converted) is complex and converted == saved
    assert converted is not complex.__complex__(child)

    class Reflected(complex):
        def __rmul__(self, other):
            return ("reflected", other)

    class Declines(complex):
        def __truediv__(self, other):
            return NotImplemented

    def reflect(other):
        return "reflected"

    class Parent(complex):
        def __mul__(self, other):
            return "forward"

        __rmul__ = staticmethod(reflect)

        def __truediv__(self, other):
            return "forward"

        @classmethod
        def __rtruediv__(cls, other):
            return cls

    class Derived(Parent):
        __rmul__ = staticmethod(reflect)

    assert Parent(1) * Derived(2) == "forward"
    assert Parent(1) / Derived(2) is Derived
    assert saved * Reflected(2) == ("reflected", saved)
    assert complex.__mul__(Reflected(2), 3) == 6 + 0j
    assert complex.__rtruediv__(3 + 4j, 2) == 0.24 - 0.32j
    assert complex.__truediv__(saved, object()) is NotImplemented
    assert repr(complex(-0.0, -0.0) * 2) == "(-0-0j)"
    assert repr(complex(-0.0, 0.0) / -2) == "-0j"
    inf = float("inf")
    assert complex(inf, 1.0) * 2 == complex(inf, 2.0)
    assert (1e300 + 1e300j) / (1e300 + 1e300j) == 1 + 0j
    try:
        Declines(2) / 3
    except TypeError:
        pass
    else:
        raise AssertionError("ignored a declined complex division")

    class PowerChild(complex):
        def __rpow__(self, other):
            return ("reflected", other)

    class ModularChild(PowerChild):
        def __rpow__(self, other, modulus):
            if modulus == 7:
                return NotImplemented
            return other, modulus

    assert saved ** PowerChild(2) == ("reflected", saved)
    assert pow(saved, ModularChild(2), 3) == (saved, 3)
    assert pow(0.1, 1 + 0j) == 0.1 + 0j
    assert complex.__rpow__(2 + 0j, 3, None) == 9 + 0j
    assert complex.__pow__(saved, object(), 2) is NotImplemented
    assert 1j ** -3 == 1j
    assert (complex(-1, -0.0) ** 0.5).imag == -1.0
    assert (complex(-1, 0.0) ** 0.5).imag == 1.0
    assert pow(complex(float("nan"), 1), 0) == 1 + 0j
    for operands, error, message in (
        ((0j, 1j), ZeroDivisionError, "zero to a negative or complex power"),
        ((1e200 + 1j, 5), OverflowError, "complex exponentiation"),
        ((1j, 2, 3), ValueError, "complex modulo"),
        ((2, 3, Child(1j)), ValueError, "complex modulo"),
        ((2, 3, PowerChild(1j)), TypeError, "unsupported operand type(s) for ** or pow(): 'int', 'int', 'PowerChild'"),
        ((saved, ModularChild(2), 7), ValueError, "complex modulo"),
    ):
        try:
            pow(*operands)
        except error as exc:
            assert str(exc) == message
        else:
            raise AssertionError(operands)

    namespace = {}
    exec(compile("z = 2 + 1j; scaled = 2.5e1J", "<complex>", "exec"), namespace)
    assert namespace["z"] == complex(2, 1)
    assert namespace["scaled"] == complex(0, 25)

    print(abs(3+4j), abs(0j), abs(complex(float("nan"), float("inf"))))
    try:
        abs(complex(1.7e308, 1.7e308))
    except OverflowError as error:
        print(error.args)
    else:
        raise AssertionError("overflowing complex absolute value")

    class Number:
        def __complex__(self):
            events.append("complex")
            return saved

        def __float__(self):
            raise AssertionError("unexpected __float__")

    class Real:
        def __float__(self):
            events.append("float")
            return -0.0

    class Index:
        def __index__(self):
            events.append("index")
            return 17

    result = complex(Number())
    assert result == saved and result is not saved
    assert repr(complex(Real(), Index())) == "(-0+17j)"
    assert events == ["complex", "float", "index"]
    z = complex(real=1.5, imag=-2)
    assert type(z) is complex and isinstance(z, complex) and not isinstance(1, complex)
    assert type(z.real) is type(0.0) and type(z.imag) is type(0.0)
    assert complex(z) is z and complex(z.real, z.imag) == z == 1.5 - 2j
    assert not complex(-0.0, 0) and bool(1j)
    print(repr(complex(-0.0, -0.0)), repr(-1j), repr(z))

    for source in (
        "-0", "-0j", "j", "+J", "-j", "1+j", "-.5-J", "( +1_0.5e-1-2.5E+2J )",
        "\u2003(٢_٠+３j)\u00a0", "-1e-9999-1e-9999j", "INF-inFinityj", "-NaN+nanj",
    ):
        value = complex(source)
        print(repr(value), repr(value.real), repr(value.imag))

    class Text(str):
        def __complex__(self):
            raise AssertionError("str argument must be parsed")

        def __repr__(self):
            return "original-text"

    assert complex(Text("3-4j")) == 3-4j
    for source in (
        "", "()", "((1))", "1 +2j", "1+ 2j", "1e+j", "1+2", "1j+2", "2jJ",
        "1__2j", "_1", "1_.0j", "١_+2j", "1_2\x00", "1\x00_2", "∞_1", Text("_"),
        "\udc80_", "_\udc80", "4\udc80", Text("_\udc80"),
    ):
        try:
            complex(source)
        except ValueError as exc:
            print(str(exc))
        else:
            raise AssertionError("accepted an invalid complex string")

    for n in (False, 1, -1, 2.25, -0.5, 9007199254740992, 1208925819614629174706176):
        key = complex(n, 0)
        assert n == key and key == n
        assert {n: "real"}[key] == "real" and {key: "complex"}[n] == "complex"
    assert complex(9007199254740993) != 9007199254740993
    assert {z: 1}[complex(1.5, -2)] == 1 and complex(1.5, 2) not in {z: 1}
    assert {complex: 3}[type(z)] == 3

    for args in ((None,), (1, []), ([], 1)):
        try:
            complex(*args)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("accepted a nonnumeric argument")
    try:
        z.real = 7
    except AttributeError as exc:
        assert str(exc) == "readonly attribute"
    else:
        raise AssertionError("modified a complex component")
    try:
        del z.imag
    except AttributeError as exc:
        assert str(exc) == "readonly attribute"
    else:
        raise AssertionError("deleted a complex component")


class TypeOwner:
    class Invalid:
        __module__ = "custom.module"


try:
    complex(0, TypeOwner.Invalid())
except TypeError as error:
    assert error.args == ("complex() argument 'imag' must be a real number, not custom.module.TypeOwner.Invalid",)
else:
    raise AssertionError("expected a type error")
