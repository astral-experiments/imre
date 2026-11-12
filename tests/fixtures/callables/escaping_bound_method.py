from typing import Callable
class Box:
    @classmethod
    def pick(cls, n: int = 3) -> int:
        return n + 1


def outer() -> Callable[[], int]:
    saved = Box.pick
    def invoke() -> int:
        return saved()
    return invoke

print(outer()())
