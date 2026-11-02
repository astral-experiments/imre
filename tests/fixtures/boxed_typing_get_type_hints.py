from typing import get_type_hints


class Target:
    pass


class Parent:
    item: "Target"
    values: list["Target"]


class Child(Parent):
    item: bytes


def fn(a: "Target") -> None:
    pass


def current():
    assert get_type_hints(Parent) == {"item": Target, "values": list[Target]}
    assert get_type_hints(Child) == {"item": bytes, "values": list[Target]}
    assert get_type_hints(fn) == {"a": Target, "return": type(None)}
    assert get_type_hints(fn, {"Target": int}, {"Target": str})["a"] is str
    assert fn.__annotations__["a"] == "Target"
    print("resolved type hints")


if __name__ == "__main__":
    current()
