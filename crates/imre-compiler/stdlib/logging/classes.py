"""Logging classes compiled into the native program."""

from logging import Formatter, LogRecord

_default_formatter = Formatter()


class Filter:
    name: str
    nlen: int

    def __init__(self, name: str = "") -> None:
        self.name = name
        self.nlen = len(name)

    def filter(self, record: LogRecord) -> bool:
        if self.nlen == 0:
            return True
        name = record.name
        if name == self.name:
            return True
        if name is None:
            raise AttributeError("'NoneType' object has no attribute 'find'")
        if not name[:self.nlen].startswith(self.name):
            return False
        return name[self.nlen] == "."


class BufferingFormatter:
    linefmt: Formatter

    def __init__(self, linefmt: Formatter | None = None) -> None:
        self.linefmt = _default_formatter if linefmt is None else linefmt

    def formatHeader(self, records: list[LogRecord]) -> str:
        return ""

    def formatFooter(self, records: list[LogRecord]) -> str:
        return ""

    def format(self, records: list[LogRecord]) -> str:
        if not records:
            return ""
        result = self.formatHeader(records)
        for record in records:
            result += self.linefmt.format(record)
        return result + self.formatFooter(records)
