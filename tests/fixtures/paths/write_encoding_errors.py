from pathlib import Path


def exercise(newline: str, source: str) -> None:
    path = Path("encoded.txt")
    with path.open("w", encoding="iso8859-15", newline=newline) as stream:
        for _ in range(2):
            try:
                stream.write(source)
            except UnicodeEncodeError as exc:
                print(exc.args[1] is source)
                print(repr(exc))
            else:
                raise AssertionError("unencodable input")
            print(stream.write("A\n"))
    print(path.read_bytes())


for ending in ["", "\n", "\r", "\r\n"]:
    exercise(ending, "x😀y")
    exercise(ending, "x\n😀y")
