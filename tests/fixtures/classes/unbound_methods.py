from typing import Callable


class Cell:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value

    def add(self, amount: int) -> int:
        self.value += amount
        return self.value


cell = Cell(1)
print(Cell.add(cell, 2))
callback: Callable[[Cell, int], int] = Cell.add
print(callback(cell, 4))
print(cell.value)
