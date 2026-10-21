def identity(value: int) -> int:
    return value + 1000


def key(value: int) -> int:
    return -value


def positive(value: int) -> bool:
    return value < 0


def items() -> list[int]:
    return [1000]

