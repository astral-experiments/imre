from typing import Union


def modern(value: int | str) -> str:
    return str(value)


def legacy(value: Union[int, str]) -> str:
    return str(value)


def mutable(value: list[str] | list[bytes]) -> int:
    return len(value)


print(modern(3), modern("three"), legacy(4), legacy("four"))
print(mutable(["a"]), mutable([b"a", b"b"]))
