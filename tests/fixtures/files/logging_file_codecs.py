"""Compare file creation, text conversion and subsequent writes."""
import logging
from pathlib import Path


def option(name: str, value: str) -> str:
    print(name)
    return value


def file_policy(path: Path, codec: str, errors: str | None, text: str) -> None:
    with path.open("w", encoding=codec, errors=errors) as writer:
        print(writer.write("start:"))
        try:
            print(writer.write(text))
        except UnicodeEncodeError as exc:
            print(str(exc))
        except LookupError as exc:
            print(str(exc))
        print(writer.write(":end"))
        writer.flush()
    print(path.read_bytes())


def check() -> None:
    path = Path("codec.log")
    for policy in ["ignore", "replace", "backslashreplace", "xmlcharrefreplace", "strict", "unknown-handler"]:
        file_policy(path, "ascii", policy, "Å🙂a")
    file_policy(path, "latin-1", None, "àö")
    file_policy(path, "1251", "strict", "до свидания")
    with path.open("w", errors=option("errors", "strict"), newline=option("newline", "\r\n"),
                   encoding=option("encoding", "utf8")) as writer:
        print(writer.write("a\nб\n"))
    print(path.read_bytes())
    with path.open("w", encoding="utf-8-sig") as writer:
        print(writer.write(""))
        print(writer.write("one"))
        print(writer.write("two"))
    print(path.read_bytes())
    with path.open("a", encoding="utf-8-sig") as writer:
        print(writer.write("three"))
    print(path.read_bytes())
    try:
        with path.open("w", encoding="ascii", errors="bad\0name") as writer:
            writer.write("unexpected")
    except ValueError as exc:
        print(str(exc))
    print(path.read_bytes())
    try:
        with path.open("w", encoding="no-such-codec", newline="bad") as writer:
            writer.write("unexpected")
    except ValueError as exc:
        print(str(exc))
    print(path.read_bytes())
    path.write_bytes(b"old")
    try:
        with path.open("w", encoding="no-such-codec") as writer:
            writer.write("unexpected")
    except LookupError as exc:
        print(str(exc))
    print(path.read_bytes())
    path.unlink()
    handler = logging.FileHandler(path, "x", encoding="cp1251", delay=True, errors="strict")
    print(path.exists())
    record = logging.LogRecord("codec", logging.WARNING, "", 1, "до свидания", (), None)
    handler.emit(record)
    handler.close()
    print(path.read_bytes())
    path.unlink()


check()
