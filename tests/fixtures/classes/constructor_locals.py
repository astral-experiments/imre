class Base:
    label: str

    def __init__(self, label: str) -> None:
        super().__init__()
        self.label = label


def checked(value: str, events: list[str]) -> str:
    events.append(value)
    return value


class Child(Base):
    numbers: list[int]

    def __init__(self, value: str, events: list[str]) -> None:
        self.numbers = [len(value)]
        assert value, checked("empty", events)
        label = checked(value, events)
        width = self.numbers[0]
        assert width == len(label)
        super().__init__(label=label)


events: list[str] = []
print(Child("one", events).label)
try:
    Child("", events)
except AssertionError as error:
    print(str(error))
for event in events:
    print(event)


class Shadow:
    value: int

    def __init__(self, super: int) -> None:
        self.value = super


print(Shadow(42).value)
