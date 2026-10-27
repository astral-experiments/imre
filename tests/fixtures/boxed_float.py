def current():
    assert callable(float) and type(float) is type
    assert float.__name__ == float.__qualname__ == "float" and float.__module__ == "builtins"
    namespace = {}
    exec("alias = float", namespace)
    assert namespace["alias"] is float and namespace["__builtins__"]["float"] is float
    z = complex(1.5, -0.0)
    assert type(z.real) is float and type(z.imag) is float and isinstance(z.real, float)
    assert {float: "atomic"}[type(z.real)] == "atomic" and not isinstance(1, float)
    assert float() == 0.0 and float(True) == 1.0 and float(False) == 0.0
    negative = z.imag
    assert float(negative) is negative and repr(negative) == "-0.0"
    nan = float("NaN")
    assert float(nan) is nan

    class Weight(float):
        pass

    weight = Weight("1.5")
    assert type(weight) is Weight and isinstance(weight, float)
    assert float(weight) == 1.5 and weight + 0.5 == 2.0
    negative_weight = float.__new__(Weight, -0.0)
    assert type(negative_weight) is Weight and repr(negative_weight) == "-0.0"
    assert Weight.__new__ is float.__new__
    assert type(Weight.__new__(Weight, 2.5)) is Weight

    print(repr(float("  -Infinity")), repr(nan), repr(float(9007199254740993)))
    assert float("  ٣.١٤  ") == float(b"  3.14\t") == float(bytearray(b"3.14"))
    assert float("2_5.e-1") == 2.5 and float(9007199254740993) == 9007199254740992.0

    class Both:
        def __float__(self):
            return 12.5
        def __index__(self):
            raise AssertionError("used __index__ before __float__")

    class Indexed:
        def __index__(self):
            return 314

    class BadFloat:
        def __float__(self):
            return 2

    class BadIndex:
        def __index__(self):
            return "2"

    class Raising:
        def __float__(self):
            raise ValueError("from conversion")

    assert float(Both()) == 12.5 and float(Indexed()) == 314.0
    for kind, value in ((TypeError, None), (TypeError, 1j), (TypeError, BadFloat()),
                        (TypeError, BadIndex()), (ValueError, Raising()), (ValueError, "1_e2"),
                        (ValueError, b"123\xff"), (ValueError, b"'\x00"), (ValueError, b'"'),
                        (ValueError, bytearray(b"12x")), (ValueError, bytearray(b"'")),
                        (OverflowError, 1 << 2000)):
        try:
            float(value)
        except kind as exc:
            print(str(exc))
        else:
            raise AssertionError("accepted invalid input")
    for args, keywords in (((1, 2), {}), ((), {"x": 3})):
        try:
            float(*args, **keywords)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("accepted invalid arguments")

    class Override(float):
        def __float__(self):
            return 9.5

    class Raises(float):
        def __float__(self):
            raise ValueError("float override")

    class Wrong(float):
        def __float__(self):
            return 7

    assert float(Override(1.5)) == 9.5
    for cls, kind in ((Raises, ValueError), (Wrong, TypeError)):
        try:
            float(cls(1.5))
        except kind:
            pass
        else:
            raise AssertionError("ignored float override")
