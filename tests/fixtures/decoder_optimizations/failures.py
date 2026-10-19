"""The first missing Unicode scalar and complete UTF-8 decoding determine errors."""


def translate(mapping: dict[str, int], text: str) -> list[int]:
    return [mapping[character] for character in text]


def check() -> None:
    mapping = {"a": 1, "b": 2}
    for text in ["axby", "a'", 'a"', "a\\", "a\x00", "a\x7f", "aé", "a\u0301", "a🦀"]:
        result = [99]
        try:
            result = translate(mapping, text)
        except KeyError as error:
            print(str(error))
        print(len(result))
        print(result[0])

    for raw in [b"axby", b"a'", b"a\x00", "a🦀".encode()]:
        result = [99]
        try:
            result = [mapping[character] for character in raw.decode()]
        except KeyError as error:
            print(str(error))
        print(len(result))
        print(result[0])

    # Even a missing initial character cannot preempt decoding a later bad byte.
    for raw in [b"x\xff", b"a\xc2", b"a\xed\xa0\x80", b"a\xf4\x90\x80\x80"]:
        try:
            values = [mapping[character] for character in raw.decode()]
            print(len(values))
        except UnicodeDecodeError as error:
            print(str(error))
        except KeyError:
            print("incorrect lookup before UTF-8 decoding")

    alias = mapping
    for turn in range(8):
        alias["a"] = turn
        assert translate(mapping, "aaa")[2] == turn
        del alias["a"]
        try:
            translate(mapping, "aba")
        except KeyError as error:
            print(str(error))
        alias["a"] = -turn
        assert translate(mapping, "aaa")[1] == -turn


check()
