"""Logging for Python modules executed by the native object runtime."""
import sys
import _imre_logging as _native

CRITICAL = 50
ERROR = 40
WARNING = 30
INFO = 20
DEBUG = 10
NOTSET = 0


class _Lock:
    def __init__(self):
        self._lock = _native.lock()

    def acquire(self):
        return _native.acquire(self._lock)

    def release(self):
        _native.release(self._lock)

    def __enter__(self):
        return self.acquire()

    def __exit__(self, exc_type, exc, traceback):
        self.release()


_handler_level = _native.check_level


class _MessageRecord:
    def __init__(self, name, level, msg, args):
        self.name = name
        self.levelno = level
        self.levelname = _native.level_name(level)
        self.msg = msg
        self.args = args
        if len(args) == 1 and isinstance(args[0], dict) and args[0]:
            self.args = args[0]

    def getMessage(self):
        message = str(self.msg)
        if self.args:
            message = message % self.args
        return message


class _MessageFormatter:
    def format(self, record):
        record.message = record.getMessage()
        return record.message


_default_formatter = _MessageFormatter()
raiseExceptions = True


def _rename_handler(handler, old, new):
    if old is not None or new is not None:
        raise NotImplementedError("named handlers in runtime-loaded logging")


def _handle_error(record):
    if raiseExceptions:
        raise NotImplementedError("handler error diagnostics in runtime-loaded logging")


def _fallback(logger, level, record, found):
    if not found and level >= WARNING:
        sys.stderr.write(record.getMessage() + "\n")
        sys.stderr.flush()


class Logger:
    def __init__(self, name, level=NOTSET):
        self._native = _native.new(name, _handler_level(level))
        self.handlers = []

    @property
    def name(self):
        return _native.name(self._native)

    @property
    def level(self):
        return _native.level(self._native)

    @property
    def parent(self):
        return _native.parent(self._native)

    @property
    def propagate(self):
        return _native.propagate(self._native)

    @property
    def disabled(self):
        return _native.disabled(self._native)

    def __setattr__(self, name, value):
        if name == "propagate":
            _native.set_propagate(self._native, value)
        elif name == "disabled":
            _native.set_disabled(self._native, value)
        elif name == "level":
            _native.set_level(self._native, value)
        else:
            object.__setattr__(self, name, value)

    def setLevel(self, level):
        _native.set_level(self._native, _handler_level(level))

    def getEffectiveLevel(self):
        return _native.effective(self._native)

    def isEnabledFor(self, level):
        return _native.enabled(self._native, level)

    def addHandler(self, hdlr):
        if hdlr not in self.handlers:
            self.handlers.append(hdlr)

    def removeHandler(self, hdlr):
        if hdlr in self.handlers:
            self.handlers.remove(hdlr)

    def log(self, level, msg, *args):
        if self.isEnabledFor(level):
            record = _MessageRecord(self.name, level, msg, args)
            _deliver(self, level, record)

    def info(self, msg, *args):
        self.log(INFO, msg, *args)

    def debug(self, msg, *args):
        self.log(DEBUG, msg, *args)

    def warning(self, msg, *args):
        self.log(WARNING, msg, *args)

    def error(self, msg, *args):
        self.log(ERROR, msg, *args)

    def critical(self, msg, *args):
        self.log(CRITICAL, msg, *args)


def getLogger(name=None):
    return _native.get(name)


root = getLogger()
