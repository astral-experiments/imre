import typing
from typing import assert_type


def current():
    value = []
    assert typing.assert_type is assert_type
    assert assert_type(value, object()) is value
    events = []

    def mark(label):
        events.append(label)
        return value

    assert assert_type(mark("value"), mark("type")) is value
    assert events == ["value", "type"]

    def fail():
        raise ValueError("type expression failed")

    try:
        assert_type(value, fail())
    except ValueError as error:
        assert str(error) == "type expression failed"
    else:
        raise AssertionError("second argument wasn't evaluated")

    for args, kwargs in (((), {}), ((value,), {}), ((value, int, str), {}),
                         ((), {"val": value, "typ": int}), ((value,), {"typ": int})):
        try:
            assert_type(*args, **kwargs)
        except TypeError:
            pass
        else:
            raise AssertionError("accepted invalid assert_type arguments")
    print("assert_type")
