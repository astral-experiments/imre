import logging
from pathlib import Path


def write(path: Path, encoding: str | None, delay: bool) -> None:
    handler = logging.FileHandler(path, "x", encoding=encoding, errors="strict", delay=delay)
    print(handler.encoding)
    print(handler.encoding is encoding)
    print(path.exists())
    record = logging.LogRecord("codec", logging.WARNING, "", 1, "ready", (), None)
    handler.emit(record)
    stream = handler.stream
    assert stream is not None
    print(stream.encoding)
    print(stream.errors)
    try:
        print(stream.write("\u00e9"))
    except UnicodeEncodeError as exc:
        print(str(exc))
    handler.close()
    print(stream.encoding)
    print(path.read_bytes())
    path.unlink()


def run() -> None:
    path = Path("handler-policy.txt")
    for delay in [False, True]:
        write(path, None, delay)
        write(path, "locale", delay)
        write(path, "utf8", delay)


run()
