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


class Shadow:
    value: int

    def __init__(self, super: int) -> None:
        self.value = super


class MappingFixture:
    reference: dict[str, str]
    key: str

    def __init__(self, key: str, value: str):
        self.reference = {}
        self.key = key
        self.reference[self.key] = value


events: list[str] = []
print(Child("one", events).label)
try:
    Child("", events)
except AssertionError as error:
    print(str(error))
for event in events:
    print(event)
print(Shadow(42).value)
print(MappingFixture("key", "value").reference["key"])
