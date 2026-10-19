# ruff: noqa: PLW0120
# Every tested condition assigns its optional local before choosing an exit.


def direct(value: int | None) -> int:
    while value := 0:
        pass
    else:
        return value + 1


def boolean(value: int | None, stop: bool) -> int:
    while (value := 0) or stop:
        break
    else:
        return value + 2
    return value + 3


def conditional(value: str | None, empty: bool) -> str:
    while (value := "") if empty else (value := "filled"):
        break
    else:
        return value + "exhausted"
    return value + "-break"


print(direct(None))
print(direct(9))
print(boolean(None, False))
print(boolean(9, True))
print(conditional(None, True))
print(conditional("before", False))
