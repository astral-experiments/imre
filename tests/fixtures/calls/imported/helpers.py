def label(number: int = 7, *, text: str = "imported") -> str:
    return f"{text}:{number}"


class Item:
    number: int

    def __init__(self, *, number: int = 9) -> None:
        self.number = number

    def label(self, *, prefix: str = "item") -> str:
        return f"{prefix}:{self.number}"


def marked(*, mark: str = "marker:\t", shift: int = -100_000_000_000_000_000_000) -> str:
    return mark + str(shift)
