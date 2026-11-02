from collections import namedtuple

Pair = namedtuple("Pair", ["first", "second"])


def is_plain(value: object) -> bool:
    return type(value) is tuple


def is_other(value: object) -> bool:
    return tuple is not type(value)


def check() -> None:
    mapping: dict[str, object] = {"key": (1, 2)}
    alias = mapping
    original = mapping["key"]
    assert is_plain(original)
    alias["key"] = Pair(1, 2)
    assert not is_plain(mapping["key"])
    assert is_other(mapping["key"])
    assert is_plain(original)
    assert not is_other(())
    assert is_other("tuple")


check()
