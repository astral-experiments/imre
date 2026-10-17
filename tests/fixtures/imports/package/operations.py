def twice(value: int) -> int:
    return add(value, value)


def label() -> str:
    return __name__


def append_and_read(left: list[int], right: list[int], value: int) -> int:
    left.append(value)
    return right[0]


# Function globals resolve when called, after this module has finished binding imports.
from arithmetic import add
