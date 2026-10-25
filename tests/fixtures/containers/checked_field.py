class Item:
    name: str
    def __init__(self, name: str) -> None:
        self.name = name
    def show(self) -> str:
        return self.name


class Box:
    item: Item | None
    def __init__(self, item: Item | None) -> None:
        self.item = item


def replace(box: Box, value: Item | None) -> None:
    box.item = value


def read(box: Box, alias: Box, value: Item | None, method: bool) -> None:
    if box.item is not None:
        replace(alias, value)
        try:
            if method:
                print(box.item.show())
            else:
                print(box.item.name)
        except AttributeError as error:
            print(str(error))


first = Box(Item("initial"))
second = Box(Item("other"))
read(first, second, None, False)
read(first, first, Item("new"), True)
read(first, first, None, False)
read(second, first, None, True)
read(Box(Item("final")), second, None, True)
last = Box(Item("method"))
read(last, last, None, True)
