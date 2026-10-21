"""Dictionary values remain arbitrary owned integers, including cached zero."""


def translate(mapping: dict[str, int], text: str) -> list[int]:
    return [mapping[character] for character in text]


def translate_bytes(mapping: dict[str, int], data: bytes) -> list[int]:
    return [mapping[character] for character in data.decode()]


def show(values: list[int]) -> None:
    print(len(values))
    for value in values:
        print(value)


def check() -> None:
    huge = (1 << 180) + 37
    mapping = {"a": 0, "b": -1, "c": huge, "d": -huge}
    show(translate(mapping, "abccddbaac"))
    show(translate(mapping, ""))
    show(translate_bytes(mapping, b"abccddbaac"))
    show(translate_bytes({}, b""))

    # Results own heap integers independently of later replacement or removal.
    retained = translate(mapping, "ccddcc")
    retained_bytes = translate_bytes(mapping, b"ccddcc")
    alias = mapping
    alias["c"] = 17
    del alias["d"]
    alias["d"] = 23
    show(retained)
    show(retained_bytes)
    show(translate(mapping, "ccddcc"))
    show(translate_bytes(mapping, b"ccddcc"))
    alias["c"] = -huge
    alias["d"] = huge
    show(translate(mapping, "ccddcc"))

    # A lookup cache must distinguish NUL, ASCII boundaries, and Unicode scalars.
    alphabet = "\x00\x01\x1f '" + '"' + "\\\x7fé\u0301🦀\U000e0100"
    symbols = {character: index for index, character in enumerate(alphabet)}
    show(translate(symbols, alphabet + alphabet[::-1]))
    show(translate_bytes(symbols, alphabet.encode()))
    symbols["é"] = -huge
    symbols["\x00"] = huge
    show(translate(symbols, "é\x00é\x00"))
    show(translate_bytes(symbols, "é\x00é\x00".encode()))

    # Failed comprehensions release previously acquired heap integer values.
    for turn in range(64):
        try:
            if turn % 2 == 0:
                translate(mapping, "ccdd?")
            else:
                translate_bytes(mapping, b"ccdd?")
        except KeyError as error:
            assert str(error) == "'?'"
        show(translate(mapping, "cc"))


check()
