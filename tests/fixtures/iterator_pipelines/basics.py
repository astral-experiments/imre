# ruff: noqa: C400, PLW0120
# Explicit generators and exhaustion suites are the operations under test.


def show(values: list[int]) -> None:
    for value in values:
        print(value)


def contexts() -> None:
    rows: list[list[str]] = list([] for index in range(1))
    print(len(rows))
    print(len(rows[0]))
    rows[0].append("contextual string")
    print(rows[0][0])
    pairs: list[tuple[int, str | None]] = list(enumerate([None, "x"]))
    for index, value in pairs:
        print(index)
        if value is None:
            print("absent")
        else:
            print(value)


contexts()
show(list(value * value for value in range(6) if value % 2))
print(sum(value * value for value in range(6) if value % 2))
print(sum((value for value in range(0)), start=9))
print(any(value for value in range(0)))
print(all(value for value in range(0)))
print(any(value for value in [0, 0, 3]))
print(all(value for value in [1, 2, 3]))
for number in (value + 10 for value in range(3)):
    print(number)

pairs = list(zip([1, 2, 3], "aé"))
for number, text in pairs:
    print(number)
    print(text)
for (number,) in zip([4, 5], strict=True):
    print(number)
empty: list[tuple[()]] = list(zip())
print(len(empty))
print(any(zip()))
print(all(zip(strict=True)))
for ignored in zip():
    print("unexpected empty zip item")
else:
    print("empty zip exhausted")
for number, text, byte in zip([7, 8], "好🙂", b"AB", strict=True):
    print(number)
    print(text)
    print(byte)

show(list(reversed([3, 4, 5])))
show(list(reversed((6, 7, 8))))
show(list(reversed(b"\x00\x80\xff")))
show(list(reversed(range(9, -2, -3))))
empty_reversed: list[int] = list(reversed(()))
print(len(empty_reversed))
for character in reversed("aé好🙂"):
    print(character)

print(any([False, False]))
print(all([True, True]))
print(any(""))
print(all("aé"))
print(any(b"\x00\x01"))
print(all(b"\x01\x00"))
print(any(["", "yes"]))
print(all(["yes", ""]))
print(any([0.0, -0.0, float("nan")]))
print(all([float("nan"), -0.0]))
optional_values: list[int | None] = [None, 0, 8]
print(any(optional_values))
print(all(optional_values))
print(any([[], [1]]))
print(all([[1], []]))
print(any({0: 1, 2: 3}))
print(all({1: 2, 0: 3}))
print(any({0}))
print(all({1, 2}))
print(any(()))
print(all(()))

large = 1000000000000000000000000000000
print(sum(value for value in range(large, large + 4)))
print(sum(reversed(range(large, large + 4))))
for index, value in enumerate((item for item in [4, 5]), large):
    print(index)
    print(value)
