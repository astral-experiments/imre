"""Clause, filter, element and dictionary key/value effects retain Python order."""


class Names:
    value: str

    def __init__(self, value: str) -> None:
        self.value = value


def source(log: list[int]) -> list[int]:
    log.append(1)
    return [1, 2, 3]


def children(log: list[int], value: int) -> list[int]:
    log.append(20 + value)
    return [0, 1, 2]


def predicate(log: list[int], marker: int, accepted: bool) -> bool:
    log.append(marker)
    return accepted


def element(log: list[int], outer: int, inner: int) -> int:
    log.append(1000 + outer * 10 + inner)
    return outer * 10 + inner


def key(log: list[int], value: int) -> int:
    log.append(100 + value)
    return value % 2


def mapped(log: list[int], value: int) -> int:
    log.append(200 + value)
    return value + 10


def held_key(names: Names, log: list[int], value: int) -> str:
    log.append(300 + value)
    return names.value


def replace_key(names: Names, log: list[int], value: int) -> int:
    log.append(400 + value)
    names.value = "replacement key with separate ownership"
    return value


def event(log: list[int], value: int) -> None:
    log.append(500 + value)


def main() -> None:
    log: list[int] = []
    values = [
        element(log, outer, inner)
        for outer in source(log)
        if predicate(log, 10 + outer, outer != 2)
        for inner in children(log, outer)
        if predicate(log, 100 + outer * 10 + inner, inner != 0)
        if predicate(log, 200 + outer * 10 + inner, inner != 2)
    ]
    for value in values:
        print(value)
    for marker in log:
        print(marker)

    log = []
    values = [
        value
        for value in range(3)
        if predicate(log, 10 + value, value > 0)
        and predicate(log, 20 + value, value < 2)
        or predicate(log, 30 + value, value == 0)
    ]
    for value in values:
        print(value)
    for marker in log:
        print(marker)

    log = []
    mapping = {key(log, value): mapped(log, value) for value in range(4)}
    for number, value in mapping.items():
        print(number)
        print(value)
    for marker in log:
        print(marker)

    log = []
    unique = {key(log, value) for value in range(4)}
    print(len(unique))
    print(0 in unique)
    print(1 in unique)
    for marker in log:
        print(marker)

    log = []
    names = Names("initial key with separate ownership")
    captured = {
        held_key(names, log, value): replace_key(names, log, value)
        for value in range(3)
    }
    for name, value in captured.items():
        print(name)
        print(value)
    for marker in log:
        print(marker)

    log = []
    nothing: list[None] = [event(log, value) for value in range(3)]
    absent: dict[int, None] = {value: event(log, value + 3) for value in range(2)}
    print(len(nothing))
    print(nothing[0] is None)
    print(absent[1] is None)
    for marker in log:
        print(marker)


main()
