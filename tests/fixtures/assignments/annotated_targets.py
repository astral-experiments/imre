class Item:
    def __init__(self, value: int) -> None:
        self.value = value

    def update(self, value: int = 10) -> None:
        self.value: int
        self.value: int = value + self.value

item = Item(2)
item.update()
item.update(3)
print(item.value)
