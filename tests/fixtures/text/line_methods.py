"""Retained endings, Python whitespace, option truth, and exception ordering."""


class Flag:
    def __init__(self, value: int):
        self.value = value

    def __bool__(self) -> bool:
        print("truth")
        if self.value < 0:
            raise RuntimeError("cannot test flag")
        return self.value != 0


class PlainFlag:
    def __init__(self, value: int):
        self.value = value


def source(text: str) -> str:
    print("receiver")
    return text


def argument(value: int) -> Flag:
    print("argument")
    return Flag(value)


def later() -> int:
    print("later")
    raise ValueError("later failure")


for text in ["", "a", "\r\n", "\r\r\n\n", "\x0b\x0c\x1c\x1d\x1e\x1f",
             "é\x85☀\u2028🪴\u2029end", "\r\n\x85", "a\nlast", "x\x00y\t"]:
    print(text.splitlines())
    print(text.splitlines(True))
    print(text.splitlines(keepends=False))
    print(text.isspace())
for text in ["\t\n\x0b\x0c\r", "\x1c\x1d\x1e\x1f", "\x85\xa0\u1680\u2029\u3000",
             "\u2007", "\u200b", "\ufeff", " \x1fa", "\U00010401"]:
    print(text.isspace())

print("x\n".splitlines(0))
print("x\n".splitlines(1 << 100))
print("x\n".splitlines(-(1 << 100)))
print("x\n".splitlines(None))
print("x\n".splitlines(0.0))
print("x\n".splitlines(0.5))
print("x\n".splitlines(""))
print("x\n".splitlines("false"))
print("x\n".splitlines(b""))
print("x\n".splitlines(b"false"))
print("x\n".splitlines([0]))
print("x\n".splitlines((False,)))
print("x\n".splitlines({"yes": 0}))
print("x\n".splitlines({0}))
empty_list: list[int] = []
empty_dict: dict[str, int] = {}
empty_set: set[int] = set()
print("x\n".splitlines(keepends=empty_list))
print("x\n".splitlines(empty_dict))
print("x\n".splitlines(empty_set))
print("x\n".splitlines(PlainFlag(0)))

print(source("a\r\n").splitlines(keepends=argument(1)))
print(source("a\r\n").splitlines(argument(0)))
try:
    print(source("").splitlines(keepends=argument(-1)))
except RuntimeError as err:
    print(err)
try:
    print(source("x").splitlines(argument(-1), later()))
except ValueError as err:
    print(err)
try:
    print(source("x").splitlines(argument(-1), keepends=argument(1)))
except TypeError as err:
    print(err)
try:
    print("".splitlines(keepends=argument(1), wrong=argument(-1)))
except TypeError as err:
    print(err)
try:
    print("".splitlines(other=argument(-1)))
except TypeError as err:
    print(err)
try:
    print(source("x").isspace(argument(-1)))
except TypeError as err:
    print(err)
try:
    print("".isspace(wrong=argument(-1)))
except TypeError as err:
    print(err)
try:
    print(source("A").isupper(argument(-1), wrong=argument(1)))
except TypeError as err:
    print(err)
try:
    print(source("A").isupper(argument(-1), later()))
except ValueError as err:
    print(err)
