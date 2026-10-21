def decode(value: bytes) -> None:
    try:
        print(value.decode())
    except UnicodeDecodeError as error:
        print(error)


def construct(count: int) -> None:
    try:
        print(bytes(count))
    except ValueError as error:
        print(error)
    except OverflowError as error:
        print(error)


def repeat(value: bytes, count: int) -> None:
    try:
        print(value * count)
    except OverflowError as error:
        print(error)


for invalid in [
    b"\x80",
    b"\xff",
    b"\xc0\xaf",
    b"\xed\xa0\x80",
    b"\xf4\x90\x80\x80",
    b"\xc2",
    b"\xe2\x82",
    b"\xf0\x9f\x8c",
    b"x\xc2y",
    b"ab\xe2\x82z",
    b"abc\xf0\x9f\x8cz",
    b"x\xf0\x9f",
]:
    decode(invalid)
for count in [
    -1,
    -9223372036854775808,
    -9223372036854775809,
    9223372036854775808,
    1000000000000000000000000000000,
]:
    construct(count)
for count in [-9223372036854775809, 9223372036854775808]:
    repeat(b"", count)
    repeat(b"a", count)
repeat(b"ab", 9223372036854775807)
for value in [-1, 256, 1000000000000000000000000000000]:
    try:
        print(bytes([1, 2, value, 4]))
    except ValueError as error:
        print(error)
for value in [-4, 3, -1000000000000000000000000000000, 1000000000000000000000000000000]:
    try:
        print(b"abc"[value])
    except IndexError as error:
        print(error)
try:
    print(b""[0])
except IndexError as error:
    print(error)
try:
    print(b"\xff".decode())
except UnicodeError:
    print("UnicodeError parent")
try:
    print(b"\xff".decode())
except ValueError:
    print("ValueError parent")
