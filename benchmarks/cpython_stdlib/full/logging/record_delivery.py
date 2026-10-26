"""Observe the same LogRecord before, during, and after native handler delivery."""
import logging
from pathlib import Path
from enum import Enum, StrEnum


class Status(StrEnum):
    OK = "ok"


class Choice(Enum):
    OK = "ok"


class Capture(logging.Handler):
    records: list[logging.LogRecord]

    def __init__(self) -> None:
        super().__init__()
        self.records = []

    def emit(self, record: logging.LogRecord | None) -> None:
        if record is not None:
            self.records.append(record)


def send(logger: logging.Logger, fmt: str, values: dict[str, str]) -> None:
    logger.warning(fmt, values)


def argument(label: str) -> str:
    print(label)
    return label


def check() -> None:
    parent = logging.getLogger("original")
    parent.propagate = False
    logger = logging.getLogger("original.child")
    first = Capture()
    second = Capture()
    logger.addHandler(first)
    parent.addHandler(second)
    values = {"who": "first"}
    send(logger, "hello %(who)s", values)
    record = first.records[0]
    print(record is second.records[0])
    print(record.msg)
    print(record.getMessage())
    values["who"] = "second"
    print(record.getMessage())
    func = record.funcName
    print("<absent>" if func is None else func)
    print(Path(record.pathname).name == Path(__file__).name)
    print(record.lineno)
    logger.error("%04d %r %%", 7, "a\nb")
    print(first.records[1].msg)
    print(first.records[1].getMessage())
    print(logging.Formatter("%(args)r").format(first.records[1]))
    # A handler that retains a record without formatting must not raise.
    logger.error("incomplete %", "extra")
    print(first.records[2].msg)
    try:
        print(first.records[2].getMessage())
    except ValueError as exc:
        print(str(exc))
    logger.error("not enough %s %s", "only")
    try:
        print(first.records[3].getMessage())
    except TypeError as exc:
        print(str(exc))
    logger.warning(Path("dir/file"))
    print(first.records[4].getMessage())
    logger.warning("path: %s (%r)", Path("dir/file"), Path("dir/file"))
    print(first.records[5].getMessage())
    logger.warning("%s %r / %s %r", Status.OK, Status.OK, Choice.OK, Choice.OK)
    print(first.records[6].getMessage())
    logger.setLevel(logging.ERROR)
    logger.info("ignored %", argument("disabled"))
    print(len(first.records))
    print(len(second.records))
    orphan = logging.getLogger("orphan")
    orphan.propagate = False
    orphan.setLevel(logging.DEBUG)
    orphan.debug("unformatted %", argument("below last resort"))
    orphan.warning("last resort: %04d", 9)


check()
