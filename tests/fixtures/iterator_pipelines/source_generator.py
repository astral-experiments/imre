from collections.abc import Iterator


def rows(values: list[str], visited: list[str]) -> Iterator[tuple[str, str]]:
    print("start")
    for value in values:
        visited.append(value)
        if value == "error":
            raise ValueError("bad row")
        yield value, value + "!"


def stopping() -> Iterator[int]:
    for index in range(2):
        if index:
            raise StopIteration("inside")
        yield index


def checked(value: int) -> int:
    if value == 9:
        raise StopIteration("element")
    return value


def source() -> list[int]:
    raise StopIteration("source")


def expression_failure(mode: int) -> None:
    try:
        if mode == 0:
            values = list(checked(x) for x in [1, 9, 3])
        elif mode == 1:
            values = list(x for x in [1, 9, 3] if checked(x))
        elif mode == 2:
            values = list(x for _ in [1] for x in source())
        elif mode == 3:
            values = list(x for x in source())
        else:
            values = list(x for x in map(checked, [1, 9, 3]))
        print(repr(values))
    except RuntimeError as error:
        print(str(error))
        cause = error.__cause__
        if cause is not None:
            print(str(cause))
    except StopIteration as error:
        print(str(error))


def opened() -> Iterator[int]:
    file = open("/dev/null", "rb")
    for index in range(2):
        if index:
            print(file.closed)
        yield index


def main() -> None:
    seen: list[str] = []
    empty: list[int] = []
    arguments = (["spread"], seen)
    for row in rows(*arguments):
        print(row[1])
    for index, item in zip(empty, rows(["unobserved"], seen)):
        print(index)
    for repetition in range(128):
        for index in opened():
            break
    mapping = {row[0]: row[1] for row in rows(["a", "b", "a"], seen)}
    print(mapping["b"])
    print(len(seen))
    for row in rows(["first", "error"], seen):
        print(row[1])
        break
    values = ["one", "two"]
    for row in rows(values, seen):
        if row[0] == "one":
            values.append("three")
        print(row[0])
    try:
        for row in rows(["before", "error"], seen):
            print(row[0])
    except ValueError as error:
        print(str(error))
    try:
        for value in stopping():
            print(value)
    except RuntimeError as error:
        print(str(error))
        cause = error.__cause__
        if cause is not None:
            print(str(cause))


main()
for mode in range(5):
    expression_failure(mode)
print(repr(list(map(checked, [1, 9, 3]))))
print(repr(list(filter(checked, [1, 9, 3]))))
