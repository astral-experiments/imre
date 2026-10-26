"""Native tab stops, identity, integer conversion and CPython's evaluation order."""

from http import HTTPStatus

class Width:
    def __init__(self, value: int):
        self.value = value

    def __index__(self) -> int:
        print("index")
        return self.value


class Odd(Width):
    def __index__(self) -> int:
        print("override")
        return 3


class Wrong:
    def __index__(self) -> str:
        print("wrong index")
        return "2"


class MayReturnNone:
    def __init__(self, ok: bool):
        self.ok = ok

    def __index__(self) -> int | None:
        if self.ok:
            return 3
        return None


class Explodes:
    def __index__(self) -> int:
        print("broken index")
        raise ValueError("index failed")


class Plain:
    value: int

    def __init__(self):
        self.value = 0


def source(s: str) -> str:
    print("receiver")
    return s


def flag(n: int) -> Width:
    print("argument")
    return Width(n)


def later() -> int:
    print("later")
    raise ArithmeticError("later failed")


def show(s: str, tab: int) -> None:
    result = s.expandtabs(tab)
    print(repr(result))
    print(result is s)


def nullable(tab: int | None) -> None:
    try:
        print(repr("é\t".expandtabs(tab)))
    except TypeError as e:
        print(e)


def override(width: Width) -> None:
    print(repr("x\t".expandtabs(width)))


def union_width(width: str | bytes) -> None:
    try: print(repr("x\t".expandtabs(width)))
    except TypeError as e: print(e)


for s in ["", "abc", "世界", "\t", "\t\t", "a\r\nb\tc", "🙂\t\r\t", "e\u0301\t",
          "a\vb\t", "a\fb\t", "a\x1cb\t", "a\x85b\t", "a\u2028b\t", "\0\t"]:
    print(repr(s.expandtabs()))
    for size in [-2147483648, -1, 0, 1, 2, 4, 8, 65]:
        show(s, size)
    print(repr(s.expandtabs(tabsize=3)))

print(repr("\t\t".expandtabs(True)))
print(repr("x\t".expandtabs(False)))
print(len("x\t".expandtabs(HTTPStatus.OK)))
print("plain".expandtabs(tabsize=HTTPStatus.NOT_FOUND))
for n in [2147483647, -2147483648, 2147483648, -2147483649, 1 << 100, -(1 << 100)]:
    try: show("no tabs", n)
    except OverflowError as e: print(e)
print(repr(source("é\t").expandtabs(tabsize=flag(3))))
print(repr(source("").expandtabs(Width(4))))
override(Width(5))
override(Odd(5))
nullable(4)
nullable(None)
union_width(b"3")
union_width("3")
for ok in [True, False]:
    try: print(repr("x\t".expandtabs(MayReturnNone(ok))))
    except TypeError as e: print(e)
try: print(source("").expandtabs(Explodes()))
except ValueError as e: print(e)
try: print(source("no tabs").expandtabs(Wrong()))
except TypeError as e: print(e)
try: print(source("no tabs").expandtabs(Plain()))
except TypeError as e: print(e)
try: print("".expandtabs(1.5))
except TypeError as e: print(e)
try: print("".expandtabs("2"))
except TypeError as e: print(e)
try: print("".expandtabs([1]))
except TypeError as e: print(e)
try: print(source("x\t").expandtabs(flag(1), later()))
except ArithmeticError as e: print(e)
try: print(source("x").expandtabs(Explodes(), tabsize=flag(5)))
except TypeError as e: print(e)
try: print(source("x").expandtabs(one=flag(1), two=flag(2)))
except TypeError as e: print(e)
try: print(source("x").expandtabs(other=flag(1)))
except TypeError as e: print(e)
