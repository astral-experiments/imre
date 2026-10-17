from ..operations import twice


def label() -> str:
    return __name__


def value(number: int) -> int:
    return twice(number) + 1
