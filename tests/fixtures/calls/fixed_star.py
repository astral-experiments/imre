class Row:
    value: str

    def __init__(self, value: str) -> None:
        self.value = value

    @classmethod
    def from_fields(cls, path: str, tag: str, size: str) -> "Row":
        return cls(path + ":" + tag + ":" + size)

    def receive(self, first: int, second: str, /, *, after: int) -> str:
        return f"{self.value}:{first}:{second}:{after}"

    @staticmethod
    def add(first: int, second: int = 13) -> int:
        return first + second


def event(log: list[int], number: int) -> int:
    log.append(number)
    return number


def receiver(log: list[int]) -> Row:
    log.append(0)
    return Row("r")


def pair(log: list[int]) -> tuple[int, str]:
    log.append(1)
    return (2, "b")


def empty(log: list[int], index: int) -> tuple[()]:
    log.append(index)
    return ()


def fail(log: list[int]) -> tuple[int, str]:
    log.append(7)
    raise ValueError("star failed")


def assemble(first, middle, third, last=10) -> str:
    return f"{first}/{middle}/{third}/{last}"


def guarded(rows: list[tuple[str, str, str] | None]) -> None:
    for record_args in rows:
        if record_args is None:
            continue
        record = Row.from_fields(*record_args)
        print(record.value)


def main() -> None:
    rows: list[tuple[str, str, str] | None] = [None, ("a", "hash", "42"), None, ("record", "", "")]
    guarded(rows)
    args = ("module.py", "sha256=value", "42")
    print(Row.from_fields(*args).value)
    log: list[int] = []
    print(receiver(log).receive(after=event(log, 3), *pair(log)))
    print(repr(log))
    print(assemble(event(log, 4), *pair(log), *empty(log, 5), last=event(log, 6)))
    print(repr(log))
    print(receiver(log).add(*(17,), *empty(log, 8)))
    print(repr(log))
    try:
        receiver(log).receive(*fail(log), after=event(log, 99))
    except ValueError as error:
        print(str(error))
    print(repr(log))


main()
