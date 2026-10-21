from typing import Callable


class Item:
    text: str
    number: int

    def __init__(self, text: str, number: int) -> None:
        self.text = text
        self.number = number

    def render(self, suffix: str) -> str:
        return self.text + suffix + str(self.number)


def select(items: list[Item]) -> Callable[[str], str]:
    print("callee selected")
    return items[0].render


def replace(items: list[Item]) -> str:
    print("argument replaces receiver")
    items[0] = Item("replacement", 2)
    for number in range(32):
        items.append(Item("grown", number))
    return ":"


def main() -> None:
    items = [Item("original", 1 << 100)]
    print(select(items)(replace(items)))
    item = Item("kept", -(1 << 100))
    fn: Callable[[str], str] = item.render
    item = Item("rebound", 3)
    print(fn("!"))
    # A bound receiver is shared, so later field changes remain visible.
    other = Item("before", 4)
    changed: Callable[[str], str] = other.render
    other.text = "after"
    print(changed("?"))
    temporary: Callable[[str], str] = Item("temporary", 5).render
    print(temporary("/"))


main()
