"""Shared first-party arithmetic."""


def add(value: int, increment: int) -> int:
    return value + increment


def label() -> str:
    return __name__


if __name__ == "__main__":
    print("arithmetic script")
