def integer_keys(values: dict[int, str]) -> str:
    match values:
        case {0.5: _}:
            return "wrong positive fraction"
        case {-0.5: _}:
            return "wrong negative fraction"
        case {1e400: _}:
            return "wrong positive infinity"
        case {-1e400: _}:
            return "wrong negative infinity"
        case {1.0: first, -0.0: zero, **rest}:
            return f"{first}:{zero}:{len(rest)}"
        case _:
            return "missing"


def large_keys(values: dict[int, str]) -> str:
    match values:
        case {1e100: positive, -1e100: negative, **rest}:
            return f"{positive}:{negative}:{len(rest)}"
        case _:
            return "missing"


def rounded_key(values: dict[int, str]) -> str:
    match values:
        case {9007199254740993.0: value}:
            return value
        case _:
            return "missing"


def bytes_keys(values: dict[bytes, float]) -> str:
    match values:
        case {"tag": _}:
            return "wrong string key"
        case {b"\x74ag": 1, b"\x00\xff": value, **rest}:
            return f"{value}:{len(rest)}"
        case _:
            return "missing"


print(integer_keys({1: "one", 0: "zero", 9: "other"}))
print(integer_keys({0: "zero"}))
exact = int(1e100)
print(large_keys({exact: "positive", -exact: "negative", 0: "other"}))
print(large_keys({exact + 1: "adjacent", -exact: "negative"}))
print(rounded_key({9007199254740992: "exact"}))
print(rounded_key({9007199254740993: "adjacent"}))
print(bytes_keys({b"tag": 1.0, b"\x00\xff": -0.0, b"extra": 2.5}))
print(bytes_keys({b"tag": 1.5, b"\x00\xff": 0.0}))
