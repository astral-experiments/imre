# ruff: noqa: C400
# Materializing a generator must release partial results and captured owners.

from typing import TextIO


class Payload:
    number: int
    text: str
    values: list[int]

    def __init__(self, number: int, text: str, values: list[int]) -> None:
        self.number = number
        self.text = text
        self.values = values


def payload(index: int) -> Payload:
    if index == 2:
        raise ValueError("element failed")
    return Payload(
        1000000000000000000000000000000 + index, "owned " + str(index), [index]
    )


def open_part(prefix: str, index: int) -> TextIO:
    if index == 2:
        raise ValueError("stop opening")
    stream = open(prefix + str(index) + ".txt", "w", encoding="utf-8")  # noqa: SIM115 — transferred owner
    stream.write(prefix + str(index))
    return stream


def break_files() -> None:
    for stream in (open_part("break", index) for index in range(4)):
        print(bool(stream))
        break


def return_files() -> bool:
    for stream in reversed([open_part("return", 0), open_part("return", 1)]):
        return bool(stream)
    return False


def recover() -> None:
    for turn in range(16):
        print(any(payload(index) for index in range(4)))
        print(all(payload(index).number < 0 for index in range(4)))
        try:
            _values = list(payload(index) for index in range(4))
        except ValueError:
            pass
        try:
            _pairs = list(zip((payload(index) for index in range(2)), [1], strict=True))
        except ValueError:
            pass
        try:
            _pairs = list(zip((payload(index) for index in range(4)), [1, 2, 3]))
        except ValueError:
            pass
        values = [payload(0), payload(1)]
        try:
            _owned_pairs = list(
                zip((values.pop() for index in range(2)), reversed(values), strict=True)
            )
        except ValueError:
            pass
        values = [payload(0), payload(1)]
        for value in reversed(values):
            values.pop()
            print(value.number)
            print(value.text)
            print(value.values[0])
        values = [payload(0), payload(1)]
        for left, right in zip((value for value in values), reversed(values)):
            values.pop()
            print(left.text)
            print(right.text)


recover()
print(any(open_part("any", index) for index in range(4)))
try:
    print(all(open_part("all", index) for index in range(4)))
except ValueError as error:
    print(error)
try:
    _opened = list(open_part("list", index) for index in range(4))
except ValueError as error:
    print(error)
print(any(zip((open_part("zip", index) for index in range(4)), [1], strict=True)))
break_files()
print(return_files())
for path in [
    "any0.txt",
    "all0.txt",
    "all1.txt",
    "list0.txt",
    "list1.txt",
    "zip0.txt",
    "break0.txt",
    "return0.txt",
    "return1.txt",
]:
    with open(path, "r", encoding="utf-8") as stream:
        print(stream.read())
