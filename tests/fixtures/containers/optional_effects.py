"""Coercing an effectful None result to an optional must preserve the call."""


class Box:
    value: int | None

    def __init__(self, value: int | None) -> None:
        self.value = value

    def clear(self, log: list[int], event: int) -> None:
        log.append(event)
        self.value = None


def absent(log: list[int], event: int) -> None:
    log.append(event)


def receive(value: int | None, log: list[int]) -> bool:
    log.append(6)
    return value is None


def returned(log: list[int]) -> int | None:
    return absent(log, 7)


def implicit(log: list[int]) -> int | None:
    log.append(12)


def conditional(flag: bool, log: list[int]) -> int | None:
    if flag:
        return 17
    log.append(13)


def target(log: list[int]) -> int:
    log.append(31)
    return 0


def main() -> None:
    log: list[int] = []
    value: int | None = absent(log, 1)
    print(value is None)
    values: list[int | None] = [absent(log, 2)]
    values[target(log)] = absent(log, 3)
    values.append(absent(log, 4))
    print(receive(absent(log, 5), log))
    print(returned(log) is None)
    box = Box(absent(log, 8))
    box.value = absent(log, 9)
    value = box.clear(log, 10)
    print(value is None)
    pair: tuple[int | None, str] = (absent(log, 11), "tuple")
    print(pair[0] is None)
    print(implicit(log) is None)
    print(conditional(False, log) is None)
    present = conditional(True, log)
    if present is not None:
        print(present)
    for index in range(len(log)):
        print(log[index])

    # Builtin calls returning None carry mutations and output too.
    numbers: list[int] = []
    appended: int | None = numbers.append(42)
    print(appended is None)
    print(numbers[0])
    nones: list[None] = [None, None]
    popped: str | None = nones.pop()
    print(popped is None)
    print(len(nones))
    printed: bool | None = print(123)
    print(printed is None)


main()
