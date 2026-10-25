from dataclasses import dataclass


@dataclass
class Row:
    items: list[bool | int]


class Options:
    enabled: bool | int

    def __init__(self, enabled: bool | int = True) -> None:
        self.enabled = enabled


def show(value: bool | int | None) -> None:
    print(isinstance(value, int))
    print(isinstance(value, bool))
    print(bool(value))
    if value is None:
        print("missing")
    elif isinstance(value, bool):
        print(repr(value))
    else:
        print(repr(value))


def widen(value: bool | None) -> bool | int | None:
    return value


def retain_integer(value: bool | int | None) -> int | None:
    if isinstance(value, bool):
        return None
    return value


def widen_integer(value: int | None) -> bool | int | None:
    return value


def as_int_instance(value: bool | int | None) -> None:
    if isinstance(value, int):
        print(repr(value))
        print(isinstance(value, bool))


def compare(value: bool | int, number: int) -> None:
    print(repr(value))
    print(value == True)
    print(value != number)
    print(False <= value < number)
    print(value is True)
    print(False is not value)


def values(flag: bool, integer: int) -> list[bool | int]:
    result: list[bool | int] = [flag, integer]
    return result


options = Options()
show(options.enabled)
options.enabled = 0
show(options.enabled)
options.enabled = False
show(options.enabled)
large = 1267650600228229401496703205376
options.enabled = large
show(options.enabled)
alias = options
alias.enabled = True
as_int_instance(options.enabled)
alias.enabled = 1
as_int_instance(options.enabled)
show(widen(None))
show(widen(False))
show(widen(True))
show(widen_integer(None))
show(widen_integer(large))
show(retain_integer(False))
show(retain_integer(None))
show(retain_integer(0))
show(retain_integer(large))
for flag in (False, True):
    for n in (0, 1, 2, -1, large):
        left = values(flag, n)
        right: list[int | bool] = [n, flag]
        print(Row(left) == Row(right))
        print(repr(Row(left)))
        compare(flag, n)
        compare(n, n)
