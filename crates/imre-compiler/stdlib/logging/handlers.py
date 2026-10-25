"""Native file rotation handlers."""

import os
from logging import FileHandler, LogRecord
from pathlib import Path
from typing import TextIO


def _can_seek(stream: TextIO | None) -> bool:
    if stream is None:
        return False
    try:
        os.lseek(stream.fileno(), 0, os.SEEK_CUR)
    except OSError:
        return False
    return True


class BaseRotatingHandler(FileHandler):
    def __init__(self, filename: str | Path, mode: str, encoding: str | None = None,
                 delay: bool = False, errors: str | None = None) -> None:
        super().__init__(filename, mode, encoding, delay, errors)

    def rotation_filename(self, default_name: str) -> str:
        return default_name

    def rotate(self, source: str, dest: str) -> None:
        if Path(source).exists():
            Path(source).replace(dest)


class RotatingFileHandler(BaseRotatingHandler):
    maxBytes: int
    backupCount: int
    _can_seek: bool

    def __init__(self, filename: str | Path, mode: str = "a", maxBytes: int = 0,
                 backupCount: int = 0, encoding: str | None = None, delay: bool = False,
                 errors: str | None = None) -> None:
        super().__init__(filename, "a" if maxBytes > 0 else mode, encoding, delay, errors)
        self.maxBytes = maxBytes
        self.backupCount = backupCount
        self._can_seek = _can_seek(self.stream)

    def shouldRollover(self, record: LogRecord | None) -> bool:
        stream = self.stream
        if stream is None:
            stream = self._open()
            self.stream = stream
            self._can_seek = _can_seek(stream)
        if self.maxBytes <= 0 or not self._can_seek:
            return False
        stream.flush()
        pos = os.lseek(stream.fileno(), 0, os.SEEK_CUR)
        if pos == 0:
            return False
        if record is None:
            raise AttributeError("'NoneType' object has no attribute 'getMessage'")
        if pos + len(self.format(record) + "\n") < self.maxBytes:
            return False
        path = Path(self.baseFilename)
        return not path.exists() or path.is_file()

    def doRollover(self) -> None:
        stream = self.stream
        if stream is not None:
            stream.close()
            self.stream = None
        if self.backupCount > 0:
            for index in range(self.backupCount - 1, 0, -1):
                source = self.rotation_filename(self.baseFilename + "." + str(index))
                dest = self.rotation_filename(self.baseFilename + "." + str(index + 1))
                if Path(source).exists():
                    if Path(dest).exists():
                        os.remove(dest)
                    Path(source).replace(dest)
            dest = self.rotation_filename(self.baseFilename + ".1")
            if Path(dest).exists():
                os.remove(dest)
            self.rotate(self.baseFilename, dest)
        if not self.delay:
            self.stream = self._open()
            self._can_seek = _can_seek(self.stream)

    def emit(self, record: LogRecord) -> None:
        try:
            if self.shouldRollover(record):
                self.doRollover()
            FileHandler.emit(self, record)
        except Exception:
            self.handleError(record)
