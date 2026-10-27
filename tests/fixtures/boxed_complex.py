def current():
    assert callable(complex) and type(complex) is type
    assert complex.__name__ == complex.__qualname__ == "complex" and complex.__module__ == "builtins"
    events = []
    saved = complex(1, 2)

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
