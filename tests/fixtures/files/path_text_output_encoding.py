from pathlib import Path


def inspect_encoding(path: Path, name: str | None) -> None:
    with path.open("w", encoding=name, errors="strict") as stream:
        spelling = stream.encoding
        print(spelling)
        print(spelling is name)
    print(stream.encoding is spelling)


def run() -> None:
    path = Path("encoding-property.txt")
    inspect_encoding(path, None)
    for name in ["locale", "utf8", "UTF-8", "ascii", "US-ASCII", "latin-1", "1251", "UTF-8-sig", "utf_8"]:
        inspect_encoding(path, name)
    try:
        inspect_encoding(path, "lOCALE")
    except LookupError as exc:
        print(str(exc))
    path.unlink()


run()
