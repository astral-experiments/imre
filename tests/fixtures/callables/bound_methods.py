from functools import update_wrapper
from typing import Callable


class Item:
    text: str
    number: int

    def __init__(self, text: str, number: int) -> None:
        self.text = text
        self.number = number

    def label(self, prefix, suffix="!"):
        return prefix + self.text + suffix

    def render(self, suffix: str) -> str:
        return self.text + suffix + str(self.number)


def repeat(text: str) -> str:
    return text + text


def select(items: list[Item]) -> Callable[[str], str]:
    print("callee selected")
    return items[0].render


def replace(items: list[Item]) -> str:
    print("argument replaces receiver")
    items[0] = Item("replacement", 2)
    for number in range(32):
        items.append(Item("grown", number))
    return ":"


def first(parts: list[str]) -> str:
    return parts[0]


def main() -> None:
    items = [Item("original", 1 << 100)]
    print(select(items)(replace(items)))
    item = Item("kept", -(1 << 100))
    fn: Callable[[str], str] = item.render
    label = item.label
    item = Item("rebound", 3)
    assert label.__self__.text == "kept"
    print(label("prefix"))
    print(label(suffix="?", prefix="before"))
    print(fn("!"))
    # A bound receiver is shared, so later field changes remain visible.
    other = Item("before", 4)
    changed: Callable[[str], str] = other.render
    other.text = "after"
    print(changed("?"))
    temporary: Callable[[str], str] = Item("temporary", 5).render
    print(temporary("/"))
    method = other.render
    wrapper = update_wrapper(repeat, method, (), ())
    saved = wrapper.__wrapped__
    update_wrapper(wrapper, temporary, (), ())
    print(wrapper is repeat, saved is method, repeat.__wrapped__ is temporary)
    print(wrapper(":"), saved("?"), wrapper.__wrapped__("/"))

    original = ":".join
    joined = update_wrapper(first, original, (), ())
    saved_join = joined.__wrapped__
    print(saved_join is original, joined(["first", "second"]), saved_join(["a", "b"]))


main()
