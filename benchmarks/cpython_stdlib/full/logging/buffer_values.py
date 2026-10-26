"""Compare native record buffering, subclass dispatch and cleanup with CPython."""
import logging.handlers
from logging import handlers
from logging.handlers import BufferingHandler


class Selective(BufferingHandler):
    flushes: int

    def __init__(self) -> None:
        super().__init__(1)
        self.flushes = 0

    def shouldFlush(self, record: logging.LogRecord | None) -> bool:
        if record is None:
            return False
        level = record.levelno
        if level is None:
            return False
        return level >= logging.WARNING

    def flush(self) -> None:
        self.flushes += 1
        super().flush()


class FailingFlush(BufferingHandler):
    fail: bool

    def __init__(self) -> None:
        super().__init__(2)
        self.fail = True

    def flush(self) -> None:
        if self.fail:
            raise RuntimeError("flush did not finish")
        super().flush()


record = logging.LogRecord("buffer", logging.INFO, "", 1, "line", (), None)
urgent = logging.LogRecord("buffer", logging.WARNING, "", 2, "flush", (), None)
empty = handlers.BufferingHandler(0)
print(empty.shouldFlush(record))
empty.emit(record)
print(len(empty.buffer))
empty.close()
one = logging.handlers.BufferingHandler(1)
print(one.shouldFlush(record))
one.close()
handler = BufferingHandler(2)
other = BufferingHandler(2)
saved = handler.buffer
print(isinstance(handler, logging.Handler))
print(saved is other.buffer)
print(handler.emit(record) is None)
print(saved[0] is record)
print(handler.shouldFlush(urgent))
handler.emit(urgent)
print(saved is handler.buffer)
print(len(saved))
handler.emit(None)
print(saved[0] is None)
handler.flush()
print(len(saved))
print(len(other.buffer))
handler.emit(record)
handler.close()
print(len(saved))
print(handler._closed)
custom = Selective()
print(custom.shouldFlush(logging.makeLogRecord({})))
custom.emit(record)
print(custom.flushes)
print(custom.buffer[0] is record)
custom.emit(urgent)
print(custom.flushes)
print(len(custom.buffer))
custom.close()
print(custom.flushes)
broken = FailingFlush()
broken.name = "failing flush"
broken.emit(record)
try:
    broken.close()
except RuntimeError as error:
    print(str(error))
print(broken._closed)
print(logging.getHandlerByName("failing flush") is None)
print(broken.buffer[0] is record)
broken.fail = False
broken.close()
print(len(broken.buffer))
other.close()
