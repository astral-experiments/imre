"""Compare rotation against the bytes and backup names left on disk."""

import logging
import logging.handlers
import os
from logging import handlers
from logging.handlers import RotatingFileHandler
from pathlib import Path


path = Path("size.log")
handler = RotatingFileHandler(path, mode="w", maxBytes=5, backupCount=2,
                              encoding="utf-8", delay=True)
print(handler.mode)
print(path.exists())
print(handler.stream is None)
small = logging.makeLogRecord({"msg": "π"})
large = logging.makeLogRecord({"msg": "abcd"})
print(handler.shouldRollover(large))
print(path.exists())
handler.emit(small)
print(handler.shouldRollover(small))
print(handler.shouldRollover(large))
handler.emit(large)
handler.emit(small)
handler.emit(large)
handler.close()
print(path.read_bytes())
print(Path("size.log.1").read_bytes())
print(Path("size.log.2").read_bytes())
print(Path("size.log.3").exists())

cursor = handlers.RotatingFileHandler("cursor.log", maxBytes=6, backupCount=1,
                                      encoding="utf-8")
cursor.emit(large)
stream = cursor.stream
if stream is not None:
    print(os.lseek(stream.fileno(), 0, os.SEEK_SET))
print(cursor.shouldRollover(large))
cursor.close()

unlimited = logging.handlers.RotatingFileHandler("unlimited.log", maxBytes=0,
                                                backupCount=1, encoding="utf-8")
unlimited.emit(large)
print(unlimited.shouldRollover(None))
unlimited.close()

# The first size decision on an existing append stream uses its end position.
Path("existing.log").write_bytes(b"old\n")
existing = RotatingFileHandler("existing.log", maxBytes=6, backupCount=1,
                               encoding="utf-8")
print(existing.shouldRollover(small))
existing.emit(small)
existing.close()
print(Path("existing.log").read_bytes())
print(Path("existing.log.1").read_bytes())
