"""Handler classes and record delivery shared by both native representations."""

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

    def createLock(self) -> None:
        self.lock = _Lock()

    def acquire(self) -> None:
        lock = self.lock
        if lock is not None:
            lock.acquire()

    def release(self) -> None:
        lock = self.lock
        if lock is not None:
            lock.release()

    def _at_fork_reinit(self) -> None:
        lock = self.lock
        if lock is None:
            raise AttributeError("'NoneType' object has no attribute '_at_fork_reinit'")
        lock._at_fork_reinit()

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

    def handle(self, record: LogRecord | None) -> LogRecord | bool | None:
        if record is None:
            return None
        for item in self.filters:
            if not item.filter(record):
                return False
        with self.lock:
            self.emit(record)
        return record

    def handleError(self, record: LogRecord) -> None:
        _handle_error(record)

    def flush(self) -> None:
        pass

    def close(self) -> None:
        self._closed = True
        _rename_handler(self, self._name, None)


class StreamHandler(Handler):
    stream: _TextOutput | None
    terminator: str

    def __init__(self, stream: _TextOutput | None = None) -> None:
        super().__init__()
        self.stream = sys.stderr if stream is None else stream
        self.terminator = "\n"

    def flush(self) -> None:
        with self.lock:
            stream = self.stream
            if stream is not None:
                stream.flush()

    def emit(self, record: LogRecord | None) -> None:
        if record is None:
            raise AttributeError("'NoneType' object has no attribute 'getMessage'")
        try:
            message = self.format(record)
            stream = self.stream
            if stream is None:
                raise AttributeError("'NoneType' object has no attribute 'write'")
            stream.write(message + self.terminator)
            self.flush()
        except RecursionError:
            raise
        except Exception:
            self.handleError(record)

    def setStream(self, stream: _TextOutput | None) -> _TextOutput | None:
        if stream is self.stream:
            return None
        with self.lock:
            self.flush()
            old = self.stream
            self.stream = stream
        return old


def _deliver(logger: Logger, level: int, record: LogRecord) -> None:
    current: Logger | None = logger
    found = False
    while current is not None:
        for handler in current.handlers:
            found = True
            if level >= handler.level:
                handler.handle(record)
        if not current.propagate:
            break
        current = current.parent
    _fallback(logger, level, record, found)
