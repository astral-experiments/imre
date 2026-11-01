import argparse


class NS:
    def __init__(self, **kwargs):
        self.__dict__.update(kwargs)

    def __eq__(self, other):
        return vars(self) == vars(other)


class Hidden:
    @property
    def __dict__(self):
        raise ValueError("unavailable")


class Slots:
    __slots__ = ()


def current():
    print([key.isidentifier() for key in ("", "_", "class", "x1", "1x", "foo.bar", "é", "a\u0301", "\U0001e6c0")])
    expected = NS(b=[1, 2], a=None)
    parser = argparse.ArgumentParser(prog="probe")
    parser.add_argument("--a")
    parser.add_argument("--b", type=int, nargs=2)
    actual = parser.parse_args(["--b", "1", "2"])
    print(repr(actual), actual == argparse.Namespace(b=[1, 2], a=None))
    print(repr(argparse.Namespace(**{"class": True, '"': False})))
    print(expected == actual, vars(actual) is actual.__dict__)
    vars(actual)["b"] = [1]
    print(expected == actual, actual.b)
    del vars(actual)["a"]
    print(hasattr(actual, "a"))
    for call in (lambda: vars(Hidden()), lambda: vars(Slots()), lambda: vars(1, 2), lambda: vars(x=1)):
        try:
            call()
        except (TypeError, ValueError) as error:
            print(type(error).__name__, str(error))
