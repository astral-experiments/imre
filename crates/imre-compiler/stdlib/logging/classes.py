"""Logging classes compiled into the native program."""

import os.path
from logging import Formatter, LogRecord
from pathlib import Path
from typing import TextIO

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


def _logfile_name(filename: str | Path) -> str:
    if isinstance(filename, Path):
        return os.path.abspath(str(filename))
    return os.path.abspath(filename)


def _open_logfile(filename: str, mode: str, encoding: str) -> TextIO:
    codec = encoding.lower().replace("_", "-")
    if codec != "utf-8" and codec != "utf8" and codec != "locale":
        raise NotImplementedError("FileHandler stream encoding: " + encoding)
    path = Path(filename)
    if mode == "a" or mode == "at" or mode == "ta":
        return path.open("a", encoding="utf-8")
    if mode == "w" or mode == "wt" or mode == "tw":
        return path.open("w", encoding="utf-8")
    if mode == "x" or mode == "xt" or mode == "tx":
        return path.open("x", encoding="utf-8")
    if mode == "r" or mode == "rt" or mode == "tr":
        return path.open("r", encoding="utf-8")
    raise NotImplementedError("FileHandler stream mode: " + mode)


class FileHandler:
    baseFilename: str
    mode: str
    encoding: str
    errors: str | None
    delay: bool
    stream: TextIO | None
    terminator: str
    formatter: Formatter | None
    _closed: bool

    def __init__(self, filename: str | Path, mode: str = "a", encoding: str | None = None,
                 delay: bool = False, errors: str | None = None) -> None:
        path = _logfile_name(filename)
        codec = "locale" if encoding is None else encoding
        self.baseFilename = path
        self.mode = mode
        self.encoding = codec
        self.errors = errors
        self.delay = delay
        self.terminator = "\n"
        self.formatter = None
        self._closed = False
        self.stream = None if delay else _open_logfile(path, mode, codec)

    def _open(self) -> TextIO:
        return _open_logfile(self.baseFilename, self.mode, self.encoding)

    def format(self, record: LogRecord) -> str:
        formatter = self.formatter
        if formatter is None:
            return _default_formatter.format(record)
        return formatter.format(record)

    def setFormatter(self, fmt: Formatter | None) -> None:
        self.formatter = fmt

    def flush(self) -> None:
        stream = self.stream
        if stream is not None:
            stream.flush()

    def emit(self, record: LogRecord) -> None:
        stream = self.stream
        if stream is None:
            if self.mode == "w" and self._closed:
                return
            stream = self._open()
            self.stream = stream
        stream.write(self.format(record) + self.terminator)
        self.flush()

    def handle(self, record: LogRecord) -> LogRecord:
        self.emit(record)
        return record

    def close(self) -> None:
        try:
            stream = self.stream
            if stream is not None:
                try:
                    stream.flush()
                finally:
                    self.stream = None
                    stream.close()
        finally:
            self._closed = True


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
