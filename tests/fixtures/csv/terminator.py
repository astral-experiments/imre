import csv


def rows(marker: str | None) -> None:
    try:
        print(repr(list(csv.reader(["a,b\r", "c\x85d\r\n", '"x\n', 'y"'], lineterminator=marker))))
    except TypeError as exc:
        print(str(exc))
    except ValueError as exc:
        print(str(exc))


def integer(value: int | None) -> None:
    try:
        csv.reader(["x"], lineterminator=value)
    except TypeError as exc:
        print(str(exc))


def source(log: list[int]) -> list[str]:
    log.append(1)
    return ["a,b"]


def option_value(log: list[int]) -> str:
    log.append(2)
    return "\n"


def main() -> None:
    for terminator in ["", "\n", "\r\n", "\x85", "\x00", "αβ", ',', '"']:
        rows(terminator)
    rows(None)
    integer(1)
    integer(None)
    try:
        csv.reader(["x"], lineterminator=False)
    except TypeError as exc:
        print(str(exc))
    try:
        csv.reader(["x"], lineterminator=b"x")
    except TypeError as exc:
        print(str(exc))
    try:
        csv.reader(["x"], escapechar="X", lineterminator="X")
    except ValueError as exc:
        print(str(exc))
    try:
        csv.reader(["x"], quotechar="X", delimiter="X", lineterminator="X")
    except ValueError as exc:
        print(str(exc))
    try:
        csv.reader(["x"], delimiter="\n", lineterminator=None)
    except TypeError as exc:
        print(str(exc))
    with open("closed", "w", encoding="utf-8") as stream:
        stream.write("line")
    with open("closed", "r", encoding="utf-8") as closed:
        pass
    try:
        csv.reader(closed, lineterminator=None)
    except ValueError as exc:
        print(str(exc))
    log: list[int] = []
    reader = csv.reader(source(log), delimiter=",", quotechar='"', lineterminator=option_value(log))
    print(repr(log))
    print(reader.line_num)
    print(repr(next(reader)))


main()
