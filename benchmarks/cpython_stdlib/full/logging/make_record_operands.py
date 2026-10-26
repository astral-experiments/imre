"""Compare retained enum, path and integer fields with CPython."""
import logging
from enum import Enum, StrEnum
from pathlib import Path, PureWindowsPath


class Plain(Enum):
    EMPTY = ""


class Text(StrEnum):
    EMPTY = ""
    WORD = "éclair"


class OtherText(StrEnum):
    WORD = "éclair"


def integer(record: logging.LogRecord, value: bool | int) -> None:
    record.__dict__["value"] = value
    boxed = record.__dict__["value"]
    print(str(boxed))
    print(bool(boxed))
    print(boxed == 0)


def check() -> None:
    record = logging.makeLogRecord({"name": "fields"})
    fields = record.__dict__
    fields["plain"] = Plain.EMPTY
    fields["empty"] = Text.EMPTY
    fields["word"] = Text.WORD
    fields["other"] = OtherText.WORD
    for key in ["plain", "empty", "word"]:
        member = fields[key]
        print(str(member))
        print(repr(member))
        print(bool(member))
        print(member == "")
        try:
            print(len(member))
            print(str(member[-1]))
        except (IndexError, TypeError) as exc:
            print(str(exc))
    print(fields["word"] == fields["other"])
    print(fields["plain"] == fields["empty"])
    print(fields["word"] == "éclair")
    print("éclair" == fields["word"])
    fields["local"] = Path("logs") / "today"
    fields["windows"] = PureWindowsPath("logs/today")
    print(logging.Formatter("%(plain)s / %(word).2s / %(local)s").format(record))
    for key in ["local", "windows"]:
        path = fields[key]
        print(str(path))
        print(repr(path))
        print(bool(path))
        try:
            print(len(path))
        except TypeError as exc:
            print(str(exc))
    integer(record, 0)
    integer(record, 1 << 90)
    integer(record, False)


check()
