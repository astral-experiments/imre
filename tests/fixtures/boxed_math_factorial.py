from importlib import import_module
from math import factorial


def current():
    assert import_module("math").factorial is factorial
    assert factorial(False) == factorial(0) == 1
    assert type(factorial(True)) is int
    calls = []

    class Indexed:
        def __index__(self):
            calls.append(42)
            return 42

    assert factorial(Indexed()) == 42 * factorial(41)
    assert calls == [42]
    try:
        factorial(Indexed(), Indexed())
    except TypeError:
        pass
    else:
        raise AssertionError("accepted two arguments")
    assert calls == [42]
    try:
        factorial(-(1 << 100))
    except ValueError:
        pass
    else:
        raise AssertionError("accepted a negative integer")
    print("factorial ok")


if __name__ == "__main__":
    current()
