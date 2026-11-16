from typing import Callable


def outer(x: int) -> Callable[[int], int]:
    def add(y: int) -> int:
        return x + y

    return add


first = outer(7)
second = outer(-2)
print(first(5), second(5), first(4), second(-3))
