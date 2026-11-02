from functools import partialmethod


class Adder:
    def add(self, left: int, right: int) -> int:
        return left + right

    increment = partialmethod(add, 1)


assert Adder().increment(41) == 42
print(Adder().increment(9))
