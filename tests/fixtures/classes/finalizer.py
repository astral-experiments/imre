class Cell:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value

    def add(self, amount: int) -> int:
        self.value += amount
        return self.value

    def __del__(self) -> None:
        print("finalized", self.value)
item = Cell(7)
del item
print("done")
