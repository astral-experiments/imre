from enum import Enum, IntEnum, IntFlag
import re


def current():
    class Label(int):
        def __repr__(self):
            return "label"

    class Number(IntEnum):
        first = 1

    class Flags(IntFlag):
        a = 1
        b = 2

    calls = []

    class Payload:
        def __repr__(self):
            calls.append("repr")
            return "<payload>"

    class Word(Enum):
        x = Payload()

    class Name(str):
        def __str__(self):
            calls.append("name")
            return "X"

    class Inactive:
        def __bool__(self):
            calls.append("truth")
            return False

    Word._value_repr_ = Inactive()
    Word.x._name_ = Name("x")
    assert repr(Word.x) == "<Word.X: <payload>>" and calls == ["truth", "name", "repr"]
    assert Flags(0).__repr__() == "<Flags: 0>"
    slot = int.__repr__
    value = Label(-(1 << 180))
    bound = slot.__get__(value, Label)
    assert slot(True) == "1" and slot(False) == "0" and repr(True) == "True"
    assert repr(value) == "label" and bound.__self__ is value
    assert bound() == slot(value) == str(-(1 << 180))
    assert slot.__get__(None, int) is slot and slot.__objclass__ is int
    assert repr(Number.first) == "<Number.first: 1>" and str(Number.first) == "1"
    assert repr(Flags.a | Flags.b) == "<Flags.a|b: 3>" and str(Flags.a | Flags.b) == "3"
    assert str(re.I | re.M) == repr(re.I | re.M) == "re.IGNORECASE|re.MULTILINE"
    for args in ((), (None,), (1.5,), (3, 4)):
        try:
            slot(*args)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("invalid int repr call")


if __name__ == "__main__":
    current()
