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


def opened() -> Iterator[int]:
    file = open("/dev/null", "rb")
    for index in range(2):
        if index:
            print(file.closed)
        yield index


def main() -> None:
    seen: list[str] = []
    empty: list[int] = []
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
