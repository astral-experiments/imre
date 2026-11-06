def current():
    class Price(float):
        def __ceil__(self):
            return "custom ceil"

    price = Price(-1.25)
    assert price.__ceil__() == "custom ceil" and float.__ceil__(price) == -1
    assert type(float.__ceil__) is type(float.__floor__)
    assert float.__ceil__.__get__(0.25, float)() == 1

    print(f'{2.675:.2f}', f'{-0.0:+.0%}', f'{True: .1f}', f'{-3:.2%}')
    print(f'{float("inf"):+f}', f'{float("nan"):.2%}')
    assert f"{2.5:03.0f}" == "002" and f"{3.5:03.0f}" == "004"
    assert "{:03.0f}".format(-0.0) == "-00" and format(True, "4.0f") == "   1"
    assert format(-1234, "03.0f") == "-1234"
    assert format(0.125, "08.0%") == "0000012%"
    assert format(float("inf"), "+07.0f") == "+000inf"
    assert format(float("nan"), " 07.0f") == " 000nan"
    for number in (1.5, 1 << 2000):
        try:
            format(number, "1" * 30 + "f")
        except ValueError:
            pass
        else:
            raise AssertionError("accepted an overflowing field width")
    try:
        print(f'{1 << 2000:.1f}')
    except OverflowError:
        print('integer too large to format as float')

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

    class Kind(str):
        pass

    hex_method = float.hex
    assert hex_method is vars(float)["hex"] and hex_method.__objclass__ is float
    assert hex_method.__text_signature__ == "($self, /)"
    for value, expected in (
        (0.0, "0x0.0p+0"),
        (-0.0, "-0x0.0p+0"),
        (-0.1, "-0x1.999999999999ap-4"),
        (5e-324, "0x0.0000000000001p-1022"),
        (2.0**-1022, "0x1.0000000000000p-1022"),
        (1.7976931348623157e308, "0x1.fffffffffffffp+1023"),
        (float("inf"), "inf"),
        (float("-inf"), "-inf"),
        (-nan, "nan"),
    ):
        assert value.hex() == expected and hex_method(value) == expected
    for args, kwargs in (((), {}), ((1,), {}), ((1.0, 2), {}), ((1.0,), {"x": 2})):
        try:
            hex_method(*args, **kwargs)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("accepted invalid hex arguments")

    assert float.fromhex(Kind("  -.8p+1\v")) == -1.0
    assert repr(float.fromhex("-1p-1075")) == "-0.0"
    assert float.fromhex("1.00000000000008p0") == 1.0
    assert float.fromhex("1.00000000000008000001p0") == 1.0 + 2**-52
    assert float.fromhex("0.fffffffffffff8p-1022") == 2.0**-1022
    try:
        float.fromhex("1.fffffffffffff8p1023")
    except OverflowError:
        pass
    else:
        raise AssertionError("hexadecimal overflow rounded to a finite value")
    desc = vars(float)["fromhex"]
    assert desc.__objclass__ is float
    assert type(desc.__get__(None, Weight)("2")) is Weight
    assert Weight().fromhex.__self__ is Weight

    class Unrelated:
        method = desc

    for receiver in (Unrelated, Unrelated()):
        try:
            receiver.method
        except TypeError:
            pass
        else:
            raise AssertionError("bound a float classmethod to an unrelated class")

    for receiver, owner in ((float, float), (0.5, float), (Weight, Weight), (Weight(), Weight)):
        method = receiver.__getformat__
        assert method.__self__ is owner
        print(method("float"), method(Kind("double")))
    for args, kwargs, kind in (((), {}, TypeError), (("float", "double"), {}, TypeError),
                               ((), {"typestr": "float"}, TypeError), ((1,), {}, TypeError),
                               (("half",), {}, ValueError), (("float\x00",), {}, ValueError),
                               (("\ud800",), {}, UnicodeEncodeError)):
        try:
            Weight.__getformat__(*args, **kwargs)
        except kind as exc:
            print(str(exc))
        else:
            raise AssertionError("accepted invalid __getformat__ argument")

    for value, ratio in (
        (-0.0, (0, 1)),
        (9.5, (19, 2)),
        (-0.1, (-3602879701896397, 1 << 55)),
        (5e-324, (1, 1 << 1074)),
        (1.7976931348623157e308, (((1 << 53) - 1) << 971, 1)),
    ):
        assert value.as_integer_ratio() == ratio
    for value, expected in ((0.0, True), (-0.0, True), (1.5, False), (5e-324, False),
                            (float(2**1023), True), (nan, False), (float("inf"), False)):
        assert value.is_integer() is expected
        assert float.is_integer(Weight(value)) is expected
    assert type(float.is_integer).__name__ == "method_descriptor"
    assert float.is_integer.__text_signature__ == "($self, /)"
    assert float.is_integer.__doc__ == "Return True if the float is an integer."
    for args, kwargs in (((), {}), ((1,), {}), ((1.0, 2), {}), ((1.0,), {"x": 2})):
        try:
            float.is_integer(*args, **kwargs)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("accepted invalid is_integer arguments")
    assert Weight(-1.5).as_integer_ratio() == (-3, 2)
    assert float.as_integer_ratio(Weight(0.5)) == (1, 2)
    for args, kwargs, kind in (((), {}, TypeError), ((1,), {}, TypeError),
                               ((0.5, 1), {}, TypeError), ((0.5,), {"x": 1}, TypeError),
                               ((nan,), {}, ValueError), ((float("-inf"),), {}, OverflowError)):
        try:
            float.as_integer_ratio(*args, **kwargs)
        except kind as exc:
            print(str(exc))
        else:
            raise AssertionError("accepted invalid as_integer_ratio call")

    for allocate in (int.__new__, float.__new__):
        for args in ((), (dict,), (1,)):
            try:
                allocate(*args)
            except TypeError as error:
                print(str(error))
            else:
                raise AssertionError("accepted invalid scalar constructor arguments")

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

    import math
    import warnings
    from array import array

    def conversion_error(source):
        try:
            float(source)
        except (TypeError, ValueError, LookupError) as exc:
            return type(exc), exc.args

    class FloatBytes(bytes):
        def __buffer__(self, flags):
            raise AssertionError("float must read bytes storage")

    class FloatByteArray(bytearray):
        def __repr__(self):
            return "original\ud800"

    for source in (FloatBytes(b" 12.5 "), FloatByteArray(b"12.5"),
                   array("B", b"12.5"), memoryview(b"x12.5y")[1:-1]):
        assert float(source) == 12.5
    for source in ("\ud8f0", Kind("1\ud800"), FloatByteArray(b"1\0"), array("B", b"x")):
        assert conversion_error(source) == (ValueError, ("could not convert string to float: " + repr(source),))

    class FloatBuffer:
        def __init__(self, view, fail_repr=False):
            self.view = view
            self.events = []
            self.fail_repr = fail_repr

        def __buffer__(self, flags):
            self.events.append(flags)
            return self.view

        def __repr__(self):
            try:
                self.view.release()
            except BufferError:
                self.events.append("repr")
            else:
                raise AssertionError("repr released an active export")
            if self.fail_repr:
                raise LookupError("repr failed")
            return "field"

        def __release_buffer__(self, view):
            assert view is self.view
            view.release()
            self.events.append("release")

    source = FloatBuffer(memoryview(b"x12.5y")[1:-1])
    assert float(source) == 12.5 and source.events == [0, "release"]
    for fail, expected in ((False, (ValueError, ("could not convert string to float: field",))),
                           (True, (LookupError, ("repr failed",)))):
        source = FloatBuffer(memoryview(b"1\xff"), fail)
        assert conversion_error(source) == expected and source.events == [0, "repr", "release"]

    released = memoryview(b"1")
    released.release()
    for view in (released, memoryview(b"123")[::2]):
        exporter = FloatBuffer(view)
        for source in (view, exporter):
            kind, (message,) = conversion_error(source)
            assert kind is TypeError and message.endswith("not '" + type(source).__name__ + "'")
        assert exporter.events == [0]

    class ReturnsFloat:
        def __init__(self, result):
            self.result = result

        def __float__(self):
            return self.result

    strict_result = ReturnsFloat(Weight(4.0))

    def convert_weight():
        return (
            float(strict_result)
        )

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always", DeprecationWarning)
        result = convert_weight()
        assert type(result) is float and result == 4.0
        assert len(caught) == 1 and caught[0].category is DeprecationWarning
        assert caught[0].filename == convert_weight.__code__.co_filename
        assert caught[0].lineno == convert_weight.__code__.co_firstlineno + 2
        assert convert_weight() is not result
        assert float(ReturnsFloat(result)) is result
        warnings.simplefilter("error", DeprecationWarning)
        try:
            math.sqrt(strict_result)
        except DeprecationWarning as error:
            assert str(error) == str(caught[0].message)
        else:
            raise AssertionError("accepted strict __float__ result with warnings-as-errors")
        assert math.sqrt(Weight(4.0)) == 2.0

    def append_weight(destination):
        (destination
         .append(strict_result))

    data = array("d", [1.0])
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        warnings.filterwarnings("error", category=DeprecationWarning,
                                lineno=append_weight.__code__.co_firstlineno + 2)
        try:
            append_weight(data)
        except DeprecationWarning:
            assert list(data) == [1.0]
        else:
            raise AssertionError("method-line warning filter did not stop append")

    display_error = TypeError("display hook")

    def fail_display(*args, **kwargs):
        raise display_error

    with warnings.catch_warnings():
        warnings.simplefilter("always", DeprecationWarning)
        warnings.showwarning = fail_display
        try:
            float(strict_result)
        except TypeError as error:
            assert error is display_error
        else:
            raise AssertionError("float lost the warning-hook exception")
        view = memoryview(data)
        try:
            view[0] = strict_result
        except TypeError as error:
            assert str(error) == "memoryview: invalid type for format 'd'"
            assert error is not display_error and view[0] == 1.0
        else:
            raise AssertionError("memoryview wrote after a conversion error")

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

    exact = float("-0")
    assert exact.real is exact and repr(exact.imag) == "0.0"
    derived = Raises("-0")
    assert derived.hex() == float.hex(derived) == "-0x0.0p+0"
    derived.__dict__["real"] = 1
    assert type(derived.real) is float and repr(derived.real) == "-0.0"
    assert derived.real is not derived.real and repr(derived.imag) == "0.0"

    assert repr(array("d", [derived])[0]) == "-0.0"
    assert repr(complex(derived).real) == "-0.0"

    class Shadow(float):
        real = "shadow"

    assert Shadow(2.5).real == "shadow"
    assert float.real.__get__(Shadow(2.5)) == 2.5
    for field in ("real", "imag"):
        descriptor = getattr(float, field)
        assert descriptor is vars(float)[field] and descriptor.__objclass__ is float
        assert descriptor.__qualname__ == "float." + field
        try:
            setattr(derived, field, 1)
        except AttributeError:
            pass
        else:
            raise AssertionError("overwrote a float component")

    assert float(Override(1.5)) == 9.5
    assert complex(Override(1.5)) == 1.5 + 0j
    assert complex(real=Override(1.5)) == 9.5 + 0j
    assert complex(Override(1.5), Override(2.5)) == 9.5 + 9.5j
    for cls, kind in ((Raises, ValueError), (Wrong, TypeError)):
        try:
            float(cls(1.5))
        except kind:
            pass
        else:
            raise AssertionError("ignored float override")
