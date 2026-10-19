"""Scalar defaults and parameter kinds bind identically through every call form."""


def describe(
    number: int = -7,
    enabled: bool = True,
    text: str = "λ\0🙂",
    nothing: None = None,
) -> str:
    return f"{number}|{enabled}|{text}|{nothing}"


def combine(left: int, /, right: int = 2, *, scale: int, offset: int = 0) -> int:
    return (left + right) * scale + offset


def huge(
    first: int = -1000000000000000000000000000000000000000000000000000000000007,
    /,
    second: int = +1000000000000000000000000000000000000000000000000000000000009,
    *,
    label: str = "adjacent literal",
) -> str:
    return f"{label}:{first}:{second}"


def keyword_only(*, value: int, enabled: bool = False) -> str:
    return f"{value}:{enabled}"


def raw_default(text: str = r"\uD800") -> str:
    return text


# Keep uppercase radix prefixes to exercise their original literal spelling.
# fmt: off
def radix_defaults(
    hexadecimal: int = -0X1_0000_0000_0000_0000,
    octal: int = -0O4000000000000000000000,
    binary: int = -0b10000000000000000000000000000000000000000000000000000000000000000,
) -> str:
    return f"{hexadecimal}:{octal}:{binary}"
# fmt: on


def optional(
    number: int | None = None,
    text: str | None = "present",
    flag: bool | None = False,
    *,
    values: list[int] | None = None,
) -> None:
    if number is None:
        print("missing number")
    else:
        print(number)
    if text is not None:
        print(text)
    if flag is not None:
        print(flag)
    if values is not None:
        print(len(values))


def main() -> None:
    print(describe())
    print(describe(8))
    print(describe(text="overridden", enabled=False, number=9))
    print(describe(10, text="mixed", nothing=None))
    print(combine(1, scale=3))
    print(combine(1, 4, offset=5, scale=6))
    print(combine(1, right=4, scale=6, offset=5))
    print(keyword_only(value=11))
    print(keyword_only(enabled=True, value=12))
    print(raw_default())
    print(radix_defaults())
    for turn in range(3):
        print(huge())
        print(huge(turn, label="changed"))
    optional()
    optional(number=1000000000000000000000000000001, text=None, flag=None, values=[])
    optional(values=[2, 3], number=4, text="specified", flag=True)


main()
