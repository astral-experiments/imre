    # Shared by the boxed and fixed-layout Handler classes.
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
        _set_handler_name(self, name)

    name = property(get_name, set_name)

    def setLevel(self, level: _Level) -> None:
        self.level = _handler_level(level)

    def setFormatter(self, fmt: Formatter | None) -> None:
        self.formatter = fmt

    def format(self, record: LogRecord) -> str:
        fmt = self.formatter
        if fmt is None:
            return _defaultFormatter.format(record)
        return fmt.format(record)

    def emit(self, record: LogRecord | None) -> None:
        raise NotImplementedError("emit must be implemented by Handler subclasses")

    def handleError(self, record: LogRecord) -> None:
        _handle_error(record)

    def flush(self) -> None:
        pass

    def close(self) -> None:
        _close_handler(self)


class NullHandler(Handler):
    def handle(self, record: LogRecord | None) -> LogRecord | bool | None:
        pass

    def emit(self, record: LogRecord | None) -> None:
        pass

    def createLock(self) -> None:
        self.lock = None

    def _at_fork_reinit(self) -> None:
        pass


class StreamHandler(Handler):
    stream: _TextOutput | None
    terminator: str

    def __init__(self, stream: _TextOutput | None = None) -> None:
        super().__init__()
        self.stream = sys.stderr if stream is None else stream
        self.terminator = "\n"

    def flush(self) -> None:
        with self.lock:
            _flush_handler_stream(self)

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
        old = self.stream
        with self.lock:
            self.flush()
            self.stream = stream
        return old
