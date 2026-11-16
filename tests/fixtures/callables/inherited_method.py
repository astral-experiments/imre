from typing import Callable


class Parent:
    def method(self, n: int = 2) -> int:
        return n + 10


class Child(Parent):
    pass


def replacement(child: Child, n: int) -> int:
    return 10 * n


def apply(fn: Callable[[Child, int], int], child: Child) -> int:
    return fn(child, 2)


child = Child()
bound = child.method
callback: Callable[[Child, int], int] = Child.method
saved = callback
callback = replacement
assert saved is Parent.method and saved is bound.__func__
assert bound.__self__ is child
assert bound is not child.method
print(apply(saved, bound.__self__), apply(callback, child), bound())
