def take(value: int) -> int:
    return value


def value():
    return 37


def text() -> int:
    return value()


print(take(text()), take(11))
