"""Logging classes compiled into the native program."""

import logging
import os.path
import sys
from logging import Formatter, Logger, LogRecord, getLevelNamesMapping, getLogger
from imre._logging import _Level, _Lock, _TextOutput, _fallback, _lookup_handler, _print_active_exception, _rename_handler
from pathlib import Path
from typing import TextIO
from logging import Logger as _AdapterLogger

_default_formatter = Formatter()
_config_lock = _Lock()


def _handle_error(record: LogRecord) -> None:
    if not logging.raiseExceptions:
        return
    try:
        sys.stderr.write("--- Logging error ---\n")
        _print_active_exception()
        sys.stderr.write("Call stack:\n")
        sys.stderr.write(f"Logged from file {record.filename}, line {record.lineno}\n")
        details = Formatter("Message: %(msg)r\nArguments: %(args)s")
        try:
            sys.stderr.write(details.formatMessage(record) + "\n")
        except RecursionError:
            raise
        except Exception:
            sys.stderr.write("Unable to print the message and arguments - possible formatting error.\n"
                             "Use the traceback above to help find the error.\n")
    except OSError:
        pass


class Filter:
    name: str
    nlen: int

    def __init__(self, name: str = "") -> None:
        self.name = name
        self.nlen = len(name)

    def filter(self, record: LogRecord) -> bool:
        if self.nlen == 0:
            return True
        name = record.name
        if name == self.name:
            return True
        if name is None:
            raise AttributeError("'NoneType' object has no attribute 'find'")
        if not name[:self.nlen].startswith(self.name):
            return False
        return name[self.nlen] == "."


def _handler_level(level: _Level) -> int:
    if isinstance(level, str):
        levels = getLevelNamesMapping()
        if level not in levels:
            raise ValueError(f"Unknown level: {level!r}")
        return levels[level]
    return level


class LoggerAdapter:
    logger: _AdapterLogger
    extra: dict[str, str] | None
    merge_extra: bool

    def __init__(self, logger: _AdapterLogger, extra: dict[str, str] | None = None, merge_extra: bool = False) -> None:
        self.logger = logger
        self.extra = extra
        self.merge_extra = merge_extra

    def process(self, msg: str, kwargs: dict[str, dict[str, str] | None]) -> tuple[str, dict[str, dict[str, str] | None]]:
        incoming = kwargs.get("extra")
        if self.merge_extra and incoming is not None:
            defaults = self.extra
            if defaults is not None:
                merged: dict[str, str] = {}
                for key in defaults:
                    merged[key] = defaults[key]
                for key in incoming:
                    merged[key] = incoming[key]
                kwargs["extra"] = merged
        else:
            kwargs["extra"] = self.extra
        return msg, kwargs

    def isEnabledFor(self, level: int) -> bool:
        return self.logger.isEnabledFor(level)

    def setLevel(self, level: _Level) -> None:
        self.logger.setLevel(_handler_level(level))

    def getEffectiveLevel(self) -> int:
        return self.logger.getEffectiveLevel()

    def hasHandlers(self) -> bool:
        return self.logger.hasHandlers()

    @property
    def name(self) -> str:
        return self.logger.name


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


def getHandlerByName(name: str) -> Handler | None:
    return _lookup_handler(name)


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


def _deliver(logger: Logger, level: int, message: str, path: str, lineno: int) -> None:
    current: Logger | None = logger
    record: LogRecord | None = None
    found = False
    while current is not None:
        for handler in current.handlers:
            found = True
            if level >= handler.level:
                if record is None:
                    record = LogRecord(logger.name, level, path, lineno, message, (), None)
                handler.handle(record)
        if not current.propagate:
            break
        current = current.parent
    _fallback(logger, level, message, found)


def _basic_config(level: int | None, fmt: str | None, datefmt: str | None, force: bool,
                  stream: _TextOutput | None, style: str, handlers: list[Handler] | None,
                  filename: str | Path | None, filemode: str, encoding: str | None, errors: str | None,
                  level_name: str | None, default_format: bool, stream_supplied: bool, filename_supplied: bool,
                  unused: str, unused_with_handlers: str) -> None:
    with _config_lock:
        root = getLogger("")
        if force:
            for old in root.handlers[:]:
                root.removeHandler(old)
                old.close()
        if root.handlers:
            return
        chosen: list[Handler]
        if handlers is None:
            if stream_supplied and filename_supplied:
                raise ValueError("'stream' and 'filename' should not be specified together")
            chosen = []
            if filename is not None and filename:
                file_errors = None if "b" in filemode else errors
                chosen.append(FileHandler(filename, filemode, encoding=encoding, errors=file_errors))
            else:
                chosen.append(StreamHandler(stream))
        else:
            unused = unused_with_handlers
            if stream_supplied or filename_supplied:
                raise ValueError("'stream' or 'filename' should not be specified together with 'handlers'")
            chosen = handlers
        if style != "%" and style != "{" and style != "$":
            raise ValueError("Style must be one of: %,{,$")
        if default_format:
            if style == "%":
                fmt = "%(levelname)s:%(name)s:%(message)s"
            elif style == "{":
                fmt = "{levelname}:{name}:{message}"
            else:
                fmt = "${levelname}:${name}:${message}"
        formatter = Formatter(fmt, datefmt, style)
        for handler in chosen:
            if handler.formatter is None:
                handler.setFormatter(formatter)
            root.addHandler(handler)
        if level_name is not None:
            root.setLevel(level_name)
        elif level is not None:
            root.setLevel(level)
        if unused:
            raise ValueError("Unrecognised argument(s): " + unused)


def _auto_config() -> None:
    if not getLogger("").handlers:
        _basic_config(None, None, None, False, None, "%", None, None, "a", None, "backslashreplace",
                      None, True, False, False, "", "")


def _logfile_name(filename: str | Path) -> str:
    if isinstance(filename, Path):
        return os.path.abspath(str(filename))
    return os.path.abspath(filename)


def _open_logfile(filename: str, mode: str, encoding: str) -> TextIO:
    codec = encoding.lower().replace("_", "-")
    if codec != "utf-8" and codec != "utf8" and codec != "locale":
        raise NotImplementedError("FileHandler stream encoding: " + encoding)
    path = Path(filename)
    if mode == "a" or mode == "at" or mode == "ta":
        return path.open("a", encoding="utf-8")
    if mode == "w" or mode == "wt" or mode == "tw":
        return path.open("w", encoding="utf-8")
    if mode == "x" or mode == "xt" or mode == "tx":
        return path.open("x", encoding="utf-8")
    if mode == "r" or mode == "rt" or mode == "tr":
        return path.open("r", encoding="utf-8")
    raise NotImplementedError("FileHandler stream mode: " + mode)


class FileHandler(StreamHandler):
    baseFilename: str
    mode: str
    encoding: str
    errors: str | None
    delay: bool

    def __init__(self, filename: str | Path, mode: str = "a", encoding: str | None = None,
                 delay: bool = False, errors: str | None = None) -> None:
        path = _logfile_name(filename)
        codec = "locale" if encoding is None else encoding
        self.baseFilename = path
        self.mode = mode
        self.encoding = codec
        self.errors = errors
        self.delay = delay
        stream = None if delay else _open_logfile(path, mode, codec)
        super().__init__(stream)
        self.stream = stream

    def _open(self) -> TextIO:
        return _open_logfile(self.baseFilename, self.mode, self.encoding)

    def emit(self, record: LogRecord | None) -> None:
        if self.stream is None:
            if self.mode == "w" and self._closed:
                return
            self.stream = self._open()
        StreamHandler.emit(self, record)

    def close(self) -> None:
        with self.lock:
            try:
                stream = self.stream
                if stream is not None:
                    try:
                        self.flush()
                    finally:
                        self.stream = None
                        stream.close()
            finally:
                StreamHandler.close(self)


class NullHandler(Handler):
    def handle(self, record: LogRecord | None) -> LogRecord | bool | None:
        pass

    def emit(self, record: LogRecord | None) -> None:
        pass

    def createLock(self) -> None:
        self.lock = None

    def _at_fork_reinit(self) -> None:
        pass


class BufferingFormatter:
    linefmt: Formatter

    def __init__(self, linefmt: Formatter | None = None) -> None:
        self.linefmt = _default_formatter if linefmt is None else linefmt

    def formatHeader(self, records: list[LogRecord]) -> str:
        return ""

    def formatFooter(self, records: list[LogRecord]) -> str:
        return ""

    def format(self, records: list[LogRecord]) -> str:
        if not records:
            return ""
        result = self.formatHeader(records)
        for record in records:
            result += self.linefmt.format(record)
        return result + self.formatFooter(records)
