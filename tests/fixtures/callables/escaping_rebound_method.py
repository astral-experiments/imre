from typing import Callable

class Base:
    @classmethod
    def pick(cls, n: int = 3) -> int:
        return n + 1

class Other:
    @classmethod
    def pick(cls, n: int = 8) -> int:
        return n + 10

def outer() -> Callable[[], int]:
    saved = Base.pick
    def invoke() -> int:
        return saved()
    saved = Other.pick
    return invoke

print(outer()())
