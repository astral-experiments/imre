class Derived(int):
    def __int__(self):
        raise AssertionError("getset coerced the subclass")


class Override(Derived):
    numerator = 3
    real = 4


def current():
    integer = int("123456789012345678901234567890")
    derived = Derived(integer)
    assert integer.numerator is integer
    assert derived.numerator == integer and type(derived.numerator) is int
    assert derived.numerator is not integer and derived.numerator is not derived.numerator
    assert type(True.numerator) is int and True.numerator == 1
    assert False.numerator == 0 and type(False.denominator) is int
    assert Override(8).numerator == 3
    assert Override(8).real == 4
    assert integer.real is integer and type(derived.real) is int
    assert True.real == 1 and type(True.real) is int
    assert False.imag == 0 and type(False.imag) is int
    for field, expected in (("numerator", integer), ("denominator", 1),
                            ("real", integer), ("imag", 0)):
        descriptor = getattr(int, field)
        assert descriptor is getattr(bool, field) and descriptor is getattr(Derived, field)
        assert type(descriptor).__name__ == "getset_descriptor"
        assert descriptor.__objclass__ is int and descriptor.__qualname__ == "int." + field
        assert descriptor.__get__(None, int) is descriptor
        derived.__dict__[field] = object()
        assert getattr(derived, field) == expected and descriptor.__get__(derived) == expected
        for method, args in ((setattr, (derived, field, 9)), (delattr, (derived, field)),
                             (descriptor.__set__, (derived, 9)), (descriptor.__delete__, (derived,))):
            try:
                method(*args)
            except AttributeError:
                pass
            else:
                raise AssertionError("wrote a read-only integer attribute")
        try:
            descriptor.__get__(object())
        except TypeError:
            pass
        else:
            raise AssertionError("descriptor accepted an unrelated receiver")

    for operation, args, kwargs, message in (
        (int.numerator.__set__, (1,), {}, "__set__ expected 2 arguments, got 1"),
        (int.numerator.__delete__, (), {}, "expected 1 argument, got 0"),
        (int.numerator.__set__, (1,), {"v": 2}, "wrapper __set__() takes no keyword arguments"),
    ):
        try:
            operation(*args, **kwargs)
        except TypeError as error:
            assert str(error) == message
        else:
            raise AssertionError("invalid descriptor call succeeded")


if __name__ == "__main__":
    current()
