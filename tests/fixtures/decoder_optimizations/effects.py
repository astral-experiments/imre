"""Iterable effects run first; filters and effectful mapping expressions still repeat."""


class Box:
    mapping: dict[str, int]

    def __init__(self, mapping: dict[str, int]) -> None:
        self.mapping = mapping


def show(values: list[int]) -> None:
    for value in values:
        print(value)


def source(mapping: dict[str, int], log: list[int], fail: bool) -> str:
    log.append(1)
    mapping["a"] = 20
    mapping["b"] = 30
    if fail:
        raise ValueError("iterable failed")
    return "abba"


def byte_source(mapping: dict[str, int], log: list[int], failure: int) -> bytes:
    log.append(4)
    mapping["a"] = 40
    mapping["b"] = 50
    if failure == 1:
        return b"x\xff"
    if failure == 2:
        raise ValueError("bytes producer failed")
    return b"abba"


def filtered(mapping: dict[str, int], log: list[int], character: str) -> bool:
    log.append(2)
    mapping[character] += 1
    return character != "b"


def selected(box: Box, log: list[int]) -> dict[str, int]:
    log.append(3)
    box.mapping["a"] += 1
    return box.mapping


def check() -> None:
    mapping = {"a": 1, "b": 2}
    log: list[int] = []
    show([mapping[character] for character in source(mapping, log, False)])
    show(log)
    try:
        values = [mapping[character] for character in source(mapping, log, True)]
        show(values)
    except ValueError as error:
        print(str(error))
    show(log)

    show([mapping[character] for character in byte_source(mapping, log, 0).decode()])
    for failure in [1, 2]:
        try:
            values = [
                mapping[character]
                for character in byte_source(mapping, log, failure).decode()
            ]
            show(values)
        except UnicodeDecodeError as error:
            print(str(error))
        except ValueError as error:
            print(str(error))
        print(mapping["a"])
        show(log)

    # An outer string with the target's spelling is evaluated before shadowing.
    character = "abba"
    show([mapping[character] for character in character])
    print(character)

    show(
        [
            mapping[character]
            for character in "abba"
            if filtered(mapping, log, character)
        ]
    )
    show(log)
    show([mapping[character] for text in ["ab", "ba"] for character in text])
    box = Box(mapping)
    show([selected(box, log)[character] for character in "aaaa"])
    show(log)

    # Typed containers with other element layouts retain ordinary lowering.
    words = {"a": "first", "b": "second"}
    for word in [words[character] for character in "abba"]:
        print(word)
    optional: dict[str, int | None] = {"a": None, "b": 9}
    for value in [optional[character] for character in "abba"]:
        if value is None:
            print("none")
        else:
            print(value)


check()
