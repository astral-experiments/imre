"""Check a retained stream, real flushed bytes, and emission after close."""

import logging
import os.path
import sys
from pathlib import Path

filename = sys.argv[1]
entry = logging.makeLogRecord({"msg": "π"})
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
