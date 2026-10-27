from numbers import Number as Numeric
import numbers
import typing as t


def is_number(value: t.Any) -> bool:
    return isinstance(value, Numeric)


class Number:
    real: int

    def __init__(self) -> None:
        self.real = 3


print(is_number(True))
print(is_number(3))
print(is_number(1 << 90))
print(is_number(1.5))
print(is_number("3"))
print(is_number(None))
print(is_number(b"3"))
print(is_number([3]))
print(isinstance(3, numbers.Number))
print(isinstance("3", (Numeric, bytes)))
print(isinstance(Number(), Numeric))
for cls in [bool, int, float, complex, str, list]:
    print(issubclass(cls, Numeric))
