from pathlib import Path


def write(path: Path, encoding: str | None, policy: str | None) -> None:
    with path.open("w", encoding=encoding, errors=policy) as stream:
        name = stream.encoding
        errors = stream.errors
        print(name)
        print(errors)
        print(name is encoding)
        print(errors is policy)
        try:
            print(stream.write("\u00e9"))
        except UnicodeEncodeError as exc:
            print(str(exc))
        except LookupError as exc:
            print(str(exc))
    print(stream.encoding is name)
    print(stream.errors is errors)
    print(path.read_bytes())


def run() -> None:
    path = Path("file-policy.txt")
    with path.open("w") as stream:
        print(stream.encoding)
        print(stream.errors)
        try:
            print(stream.write("\u00e9"))
        except UnicodeEncodeError as exc:
            print(str(exc))
    print(path.read_bytes())
    write(path, None, None)
    write(path, "locale", None)
    write(path, "UTF-8", "strict")
    for policy in ["strict", "ignore", "replace", "backslashreplace", "unknown-handler"]:
        write(path, "ascii", policy)
    for encoding in ["utf-16", "utf-32"]:
        with path.open("w", encoding=encoding) as stream:
            print(stream.write("A"), stream.write("é"))
        print(path.read_bytes())
        with path.open("a", encoding=encoding) as stream:
            print(stream.write("𝄞"))
        print(path.read_bytes())
        path.write_bytes(b"")
        with path.open("a", encoding=encoding) as stream:
            print(stream.write("B"))
        print(path.read_bytes())
    path.unlink()


run()
