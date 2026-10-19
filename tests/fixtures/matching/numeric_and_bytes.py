def integer_pattern(value: int) -> str:
    match value:
        case -0.0:
            return "zero"
        case 1.0 as original:
            return f"one:{original}"
        case 9007199254740992.0:
            return "exact binary integer"
        case 1e400 | -1e400:
            return "wrong integer infinity"
        case _:
            return "other integer"


def float_pattern(value: float) -> str:
    match value:
        case True | False:
            return "wrong singleton"
        case 0 as original:
            return f"zero:{original}"
        case 1 as original:
            return f"one:{original}"
        case 9007199254740993:
            return "wrong rounded comparison"
        case 9007199254740992:
            return "exact binary integer"
        case 0.5:
            return "half"
        case 1e400:
            return "positive infinity"
        case -1e400:
            return "negative infinity"
        case _:
            return "other float"


def bool_pattern(value: bool) -> str:
    match value:
        case 1.0 as original:
            return f"one:{original}"
        case -0.0 as original:
            return f"zero:{original}"
        case _:
            return "wrong boolean"


def optional_float(value: float | None) -> str:
    match value:
        case (None as original) | (-0.0 as original):
            if original is None:
                return "none"
            return f"optional zero:{original}"
        case original:
            return f"optional other:{original}"


def bytes_pattern(value: bytes | None) -> str:
    match value:
        case None:
            return "none"
        case "tag":
            return "wrong string"
        case b"tag" as original:
            return str(original)
        case b"\x00\xff":
            return "binary"
        case original:
            return str(original)


huge = 1
for index in range(400):
    huge *= 10
for integer_value in [0, 1, 9007199254740992, 9007199254740993, huge, -huge]:
    print(integer_pattern(integer_value))
for float_value in [
    -0.0,
    0.0,
    1.0,
    9007199254740992.0,
    0.5,
    1e400,
    -1e400,
    float("nan"),
    float("5e-324"),
]:
    print(float_pattern(float_value))
print(bool_pattern(True))
print(bool_pattern(False))
print(optional_float(None))
print(optional_float(-0.0))
print(optional_float(1.5))
print(bytes_pattern(None))
print(bytes_pattern(b"tag"))
print(bytes_pattern(b"\x00\xff"))
print(bytes_pattern(b"other"))
match (b"tag", 1.0):
    case (b"tag", 1):
        print("nested scalars")
    case _:
        print("wrong nested scalars")
