"""Logging classes compiled into the native program."""

from logging import LogRecord


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
