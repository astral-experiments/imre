"""Handler layout and record delivery for fixed-layout logging."""

class Handler:
    _name: str | None
    _closed: bool
    level: int
    formatter: Formatter | None
    filters: list[Filter]
    lock: _Lock | None

    def __init__(self, level: _Level = 0) -> None:
        self.filters = []
        self._name = None
        self.level = _handler_level(level)
        self.formatter = None
        self._closed = False
        self.createLock()

    def handle(self, record: LogRecord | None) -> LogRecord | bool | None:
        if record is None:
            return None
        for item in self.filters:
            if not item.filter(record):
                return False
        with self.lock:
            self.emit(record)
        return record
