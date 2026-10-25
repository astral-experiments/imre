import csv


def fallback(log: list[str]) -> list[str]:
    log.append("default")
    return log


def main() -> None:
    events: list[str] = []
    probe = csv.reader(["first"])
    print(repr(next(iter(probe), fallback(events))))
    print(repr(next(probe, fallback(events))))
    lines = ['a,"1,2"', '"hello\r\n', 'world",z', "x"]
    reader = csv.reader(lines)
    print(reader.line_num)
    alias = iter(reader)
    lines[0] = 'changed,"a,b"'
    print(repr(next(alias)))
    print(reader.line_num)
    print(repr(next(reader)))
    print(reader.line_num)
    lines.append("new")
    print(repr(list(alias)))
    lines.append("too late")
    print(repr(list(reader)))
    try:
        next(reader)
    except StopIteration:
        print("end")

    reader = csv.reader(['"closed"x', 'bad\nrow', '"open'], strict=True)
    for label in ["strict", "newline", "EOF"]:
        try:
            next(reader)
        except csv.Error as exc:
            print(f"{label} {reader.line_num}: {exc}")
    print(repr(list(csv.reader(['a| "b|c"|"d""e"'], delimiter="|", skipinitialspace=True))))
    print(repr(list(csv.reader(['x\\\n', 'y', 'z,0'], escapechar="\\"))))
    print(repr(list(csv.reader(['a;"quoted";🦀'], delimiter=";", quotechar=None))))
    print(repr(list(csv.reader(['"ab"c'], doublequote=False))))
    print(repr(list(csv.reader("é,🦀"))))
    print(repr(list(csv.reader(["", "", "nul\x00byte"]))))
    try:
        csv.reader(["x"], delimiter='"')
    except ValueError as exc:
        print(str(exc))
    large = csv.reader(["é" * 131072, "é" * 131073, "after"])
    print(len(next(large)[0]))
    try:
        next(large)
    except csv.Error as exc:
        print(str(exc))
    print(repr(next(large)))
    with open("input.csv", "w", encoding="utf-8") as output:
        output.write("a,b\nc,d\n")
    with open("input.csv", "r", encoding="utf-8") as stream:
        remaining = csv.reader(stream)
        print(repr(next(remaining)))
        print(stream.readline())
    try:
        next(remaining)
    except ValueError as exc:
        print(str(exc))


def quoted_line_endings() -> None:
    with open("input.csv", "wb") as binary:
        binary.write(b'"a\r\nb","c\rd"\r\nlast,row\r')
    with open("input.csv", newline="") as stream:
        reader = csv.reader(stream)
        print(repr(next(reader)))
        print(reader.line_num)
        print(repr(list(reader)))
        print(reader.line_num)


main()
quoted_line_endings()
