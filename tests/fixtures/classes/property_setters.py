class Setting:
    stored: int

    def __init__(self, value: int) -> None:
        self.stored = value

    @property
    def level(self) -> int:
        print("get", self.stored)
        return self.stored

    @level.setter
    def level(self, value: int) -> str:
        print("set", value)
        if value < 0:
            raise ValueError("negative")
        self.stored = value
        return "ignored"


class Inherited(Setting):
    pass


def receiver(item: Inherited) -> Inherited:
    print("receiver")
    return item


def value(n: int) -> int:
    print("value", n)
    if n == 0:
        raise RuntimeError("no value")
    return n


item = Inherited(2)
for n in (4, -1, 0, 3):
    try:
        receiver(item).level = value(n)
    except (ValueError, RuntimeError) as error:
        print(str(error))
    print(item.level)
