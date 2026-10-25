from functools import cached_property as remember


class Reading:
    label: str
    attempts: int

    def __init__(self, label: str) -> None:
        self.label = label
        self.attempts = 0

    @remember
    def value(self) -> str:
        self.attempts += 1
        if self.attempts == 1:
            raise ValueError("retry")
        return self.label

    @remember
    def absent(self) -> str | None:
        print("compute absent")
        return None


class Extended(Reading):
    suffix: str

    def __init__(self, label: str, suffix: str) -> None:
        super().__init__(label)
        self.suffix = suffix


def check(item: Reading) -> None:
    try:
        print(item.value)
    except ValueError as error:
        print(str(error))
    print(item.value)
    item.label = "changed"
    print(item.value)
    print(item.attempts)
    print(item.absent is None)
    print(item.absent is None)


check(Reading("first"))
child = Extended("second", "extra")
check(child)
print(child.suffix)
