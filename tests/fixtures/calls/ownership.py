"""Earlier arguments and method receivers retain roots across later keyword effects."""


class Box:
    text: str
    number: int

    def __init__(
        self, text: str, number: int = 1000000000000000000000000000007
    ) -> None:
        self.text = text
        self.number = number

    def describe(self, *, marker: int = 0) -> str:
        return f"{self.text}:{self.number}:{marker}"


class Owner:
    box: Box

    def __init__(self, box: Box) -> None:
        self.box = box


def replace(owner: Owner) -> int:
    owner.box = Box(number=0, text="replacement")
    return 8


def remove_text(values: list[str]) -> int:
    values.pop()
    return 9


def remove_number(values: list[int]) -> int:
    values.pop()
    return 10


def capture(changed: int, text: str, number: int) -> str:
    return f"{text}:{number}:{changed}"


def main() -> None:
    owner = Owner(box=Box(text="sole owner " + "field"))
    print(capture(number=owner.box.number, text=owner.box.text, changed=replace(owner)))
    owner.box = Box(text="receiver owner " + "field")
    print(owner.box.describe(marker=replace(owner)))
    print(owner.box.text)
    texts = ["sole owned " + "list item"]
    print(capture(text=texts[0], changed=remove_text(texts), number=11))
    numbers = [1000000000000000000000000000000000000000000000000000000000013]
    print(
        capture(number=numbers[0], changed=remove_number(numbers), text="saved integer")
    )
    print(len(numbers))
    items = [Box(number=index, text=f"item{index}") for index in range(3)]
    for item in items:
        print(item.describe(marker=14))


main()
