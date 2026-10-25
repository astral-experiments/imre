"""Check a retained stream, real flushed bytes, and emission after close."""

import logging
import os.path
import sys
from logging.handlers import RotatingFileHandler
from pathlib import Path

seen_records: list[logging.LogRecord] = []


class Observed(logging.FileHandler):
    def handleError(self, record: logging.LogRecord) -> None:
        active = sys.exception()
        if active is not None:
            print(record is seen_records[0])
            print(repr(active))


class BadFormat(Observed):
    def format(self, record: logging.LogRecord) -> str:
        raise ValueError("bad format")


class Recursive(Observed):
    def format(self, record: logging.LogRecord) -> str:
        raise RecursionError("recursive format")


class Redirect(logging.FileHandler):
    def format(self, record: logging.LogRecord) -> str:
        self.stream = Path(self.baseFilename + ".redirect").open("w", encoding="utf-8")
        return record.getMessage()


class BrokenRotation(RotatingFileHandler):
    def rotate(self, source: str, dest: str) -> None:
        raise ValueError("rotation")

    def handleError(self, record: logging.LogRecord) -> None:
        print(record is seen_records[0])
        print(isinstance(sys.exception(), ValueError))


filename = sys.argv[1]
entry = logging.makeLogRecord({"msg": "π"})
seen_records.append(entry)
supplied_path = Path(filename + ".supplied")
supplied_path.write_bytes(b"old\n")
rotator = RotatingFileHandler(supplied_path, maxBytes=2, backupCount=1, delay=True)
rotator.stream = supplied_path.open("a", encoding="utf-8")
print(rotator.shouldRollover(entry))
rotator.emit(entry)
rotator.close()
print(repr(supplied_path.read_bytes()))
print(repr(Path(filename + ".supplied.1").read_bytes()))

redirect_path = Path(filename + ".redirect")
delayed = logging.FileHandler(filename, mode="w", encoding="utf-8", delay=True)
print(delayed.stream is None)
print(os.path.exists(filename))
print(delayed.handle(entry) is entry)
stream = delayed.stream
print(stream is not None)
print(Path(filename).read_text(encoding="utf-8"))
delayed.close()
delayed.emit(entry)
print(delayed.stream is None)
print(Path(filename).read_text(encoding="utf-8"))
appender = logging.FileHandler(Path(filename), encoding="utf-8")
appender.terminator = "!"
appender.handle(entry)
appender.close()
appender.emit(entry)
appender.close()
print(Path(filename).read_text(encoding="utf-8"))

readonly = Observed(filename, mode="r", encoding="utf-8")
print(readonly.handle(entry) is entry)
readonly.close()
broken = BadFormat(filename, mode="a", encoding="utf-8")
print(broken.handle(entry) is entry)
broken.close()
recursive = Recursive(filename, mode="a", encoding="utf-8")
try:
    recursive.handle(entry)
except RecursionError as error:
    print(str(error))
recursive.close()
unopened = Observed(filename + ".missing/child", delay=True)
try:
    unopened.handle(entry)
except FileNotFoundError:
    print("open failed")
print(unopened.stream is None)
unopened.close()
logging.raiseExceptions = len(sys.argv) > 2
silent = logging.FileHandler(filename, mode="r", encoding="utf-8")
print(silent.handle(entry) is entry)
silent.close()

redirect = Redirect(filename, mode="a", encoding="utf-8")
original = redirect.stream
redirect.emit(entry)
redirect.close()
if original is not None:
    original.close()
print(redirect_path.read_text(encoding="utf-8"))

rotating = BrokenRotation(filename, maxBytes=1, backupCount=1)
print(rotating.handle(entry) is entry)
try:
    rotating.doRollover()
except ValueError as error:
    print(str(error))
rotating.close()
