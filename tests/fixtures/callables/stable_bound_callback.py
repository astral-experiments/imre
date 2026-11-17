from typing import Callable


class Box:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value

    def pick(self, n: int = 3) -> int:
        return self.value + n


def outer(value: int) -> Callable[[], int]:
    saved = Box(value).pick

    def invoke() -> int:
        return saved()

    alias = invoke
    print(alias is invoke)
    return alias


def object_identity(value: int) -> bool:
    saved = Box(value).pick

    def original() -> Callable[[int], int]:
        return saved

    alias = original
    return alias() is saved


first = outer(10)
second = outer(20)
print(first(), second(), first is second, object_identity(4))
