"""Logging classes compiled into the native program."""

from logging import Formatter, LogRecord, getLevelNamesMapping
from imre._logging import _Level, _lookup_handler, _rename_handler

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


def _handler_level(level: _Level) -> int:
    if isinstance(level, str):
        levels = getLevelNamesMapping()
        if level not in levels:
            raise ValueError(f"Unknown level: {level!r}")
        return levels[level]
    return level


class Handler:
    _name: str | None
    _closed: bool
    level: int
    formatter: Formatter | None
    filters: list[Filter]

    def __init__(self, level: _Level = 0) -> None:
        self.filters = []
        self._name = None
        self.level = _handler_level(level)
        self.formatter = None
        self._closed = False

    def get_name(self) -> str | None:
        return self._name

    def set_name(self, name: str | None) -> None:
        _rename_handler(self, self._name, name)
        self._name = name

    name = property(get_name, set_name)

    def setLevel(self, level: _Level) -> None:
        self.level = _handler_level(level)

    def setFormatter(self, fmt: Formatter | None) -> None:
        self.formatter = fmt

    def format(self, record: LogRecord) -> str:
        fmt = self.formatter
        if fmt is None:
            return _default_formatter.format(record)
        return fmt.format(record)

    def emit(self, record: LogRecord | None) -> None:
        raise NotImplementedError("emit must be implemented by Handler subclasses")

    def flush(self) -> None:
        pass

    def close(self) -> None:
        self._closed = True
        _rename_handler(self, self._name, None)


def getHandlerByName(name: str) -> Handler | None:
    return _lookup_handler(name)


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
