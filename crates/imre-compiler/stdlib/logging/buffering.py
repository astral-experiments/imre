"""Buffering handler shared by checked and runtime-loaded logging.handlers."""
import logging


class BufferingHandler(logging.Handler):
    capacity: int
    buffer: list[logging.LogRecord | None]

    def __init__(self, capacity: int) -> None:
        super().__init__()
        self.capacity = capacity
        self.buffer = []

    def shouldFlush(self, record: logging.LogRecord | None) -> bool:
        return len(self.buffer) >= self.capacity

    def emit(self, record: logging.LogRecord | None) -> None:
        self.buffer.append(record)
        if self.shouldFlush(record):
            self.flush()

    def flush(self) -> None:
        with self.lock:
            self.buffer.clear()

    def close(self) -> None:
        try:
            self.flush()
        finally:
            super().close()
