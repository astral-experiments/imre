"""Scalar conversions are sealed and preserve existing optional narrowing."""


def narrowed(number: int | None, text: str | None) -> None:
    if number is not None:
        print(f"number:{number!s:}")
    if text is not None:
        print(f"text:{text:}")


def main() -> None:
    print(f"{True}|{False}|{None}|{'λ\0🙂'}")
    print(f"{True!s:}|{False!s:}|{None!s:}|{'λ'!s:}")
    print(f"{Exception()}|{ValueError('message')}|{KeyError()}|{KeyError('')}")
    print(f"{KeyError("quote's")}|{KeyError('single\'"double')}")
    print(f"{KeyError('line\nzero\0é\u200b\ue000')!s:}")
    try:
        raise ValueError("caught")
    except ValueError as problem:
        print(f"caught:{problem}")
    boundaries = [
        -9223372036854775808,
        -4611686018427387905,
        -4611686018427387904,
        -1,
        0,
        1,
        4611686018427387903,
        4611686018427387904,
        9223372036854775807,
        1000000000000000000000000000000000000000000000000000000000007,
    ]
    for number in boundaries:
        print(f"{number}:{-number}:{number!s:}")
    narrowed(None, None)
    narrowed(1000000000000000000000000000000, "present")


main()
