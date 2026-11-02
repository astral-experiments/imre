"""Logging for Python modules executed by the native object runtime."""
import sys
import weakref
import _imre_logging as _native


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


_lock = _Lock()
_handlers = weakref.WeakValueDictionary()
_handlerList = []


def _removeHandlerRef(wr):
    """
    Remove a handler reference from the internal cleanup list.
    """
    # This function can be called during module teardown, when globals are
    # set to None. It can also be called from another thread. So we need to
    # pre-emptively grab the necessary globals and check if they're None,
    # to prevent race conditions and failures during interpreter shutdown.
    handlers, lock = _handlerList, _lock
    if lock and handlers:
        with lock:
            try:
                handlers.remove(wr)
            except ValueError:
                pass


def _addHandlerRef(handler):
    """
    Add a handler to the internal cleanup list using a weak reference.
    """
    with _lock:
        _handlerList.append(weakref.ref(handler, _removeHandlerRef))


def _handler_init(self, level=NOTSET):
    self.filters = []
    self._name = None
    self.level = _checkLevel(level)
    self.formatter = None
    self._closed = False
    _addHandlerRef(self)
    self.createLock()


# Typed handlers register their fixed-layout record after construction.
_handler_init.__name__ = Handler.__init__.__name__
_handler_init.__qualname__ = Handler.__init__.__qualname__
Handler.__init__ = _handler_init
del _handler_init


def shutdown(handlerList=_handlerList):
    """
    Perform any cleanup actions in the logging system (e.g. flushing
    buffers).

    Should be called at application exit.
    """
    for wr in reversed(handlerList[:]):
        #errors might occur, for example, if files are locked
        #we just ignore them if raiseExceptions is not set
        try:
            h = wr()
            if h:
                try:
                    h.acquire()
                    # MemoryHandlers might not want to be flushed on close,
                    # but circular imports prevent us scoping this to just
                    # those handlers.  hence the default to True.
                    if getattr(h, 'flushOnClose', True):
                        h.flush()
                    h.close()
                except (OSError, ValueError):
                    # Ignore errors which might be caused
                    # because handlers have been closed but
                    # references to them are still around at
                    # application exit.
                    pass
                finally:
                    h.release()
        except: # ignore everything, as we're shutting down
            if raiseExceptions:
                raise
            #else, swallow


def _handler_level(level):
    return _checkLevel(level)


class _MessageFormatter:
    def format(self, record):
        record.message = record.getMessage()
        message = "%(message)s" % record.__dict__
        if record.exc_info and not record.exc_text:
            record.exc_text = self.formatException(record.exc_info)
        for detail in (record.exc_text, record.stack_info):
            if detail:
                if not message.endswith("\n"):
                    message += "\n"
                message += detail
        return message

    def formatException(self, ei):
        """
        Format and return the specified exception information as a string.

        This default implementation just uses
        traceback.print_exception()
        """
        sio = io.StringIO()
        tb = ei[2]
        # See issues #9427, #1553375. Commented out for now.
        #if getattr(self, 'fullstack', False):
        #    traceback.print_stack(tb.tb_frame.f_back, file=sio)
        traceback.print_exception(ei[0], ei[1], tb, limit=None, file=sio)
        s = sio.getvalue()
        sio.close()
        if s[-1:] == "\n":
            s = s[:-1]
        return s


_default_formatter = _MessageFormatter()


def _flush_handler_stream(handler):
    if handler.stream and hasattr(handler.stream, "flush"):
        handler.stream.flush()


def _set_handler_name(handler, name):
    with _lock:
        if handler._name in _handlers:
            del _handlers[handler._name]
        handler._name = name
        if name:
            _handlers[name] = handler


def _close_handler(handler):
    with _lock:
        handler._closed = True
        if handler._name and handler._name in _handlers:
            del _handlers[handler._name]


def getHandlerByName(name):
    return _handlers.get(name)


def getHandlerNames():
    return frozenset(_handlers)


def _handle_error(record):
    if not raiseExceptions or not sys.stderr:
        return
    exc = sys.exception()
    try:
        sys.stderr.write("--- Logging error ---\n")
        traceback.print_exception(exc, limit=None, file=sys.stderr)
        sys.stderr.write("Call stack:\n")
        frame = exc.__traceback__.tb_frame
        while frame and os.path.dirname(frame.f_code.co_filename) == __path__[0]:
            frame = frame.f_back
        if frame:
            traceback.print_stack(frame, file=sys.stderr)
        else:
            sys.stderr.write("Logged from file %s, line %s\n" % (record.filename, record.lineno))
        try:
            sys.stderr.write("Message: %r\nArguments: %s\n" % (record.msg, record.args))
        except RecursionError:
            raise
        except Exception:
            sys.stderr.write("Unable to print the message and arguments - possible formatting error.\n"
                             "Use the traceback above to help find the error.\n")
    except OSError:
        pass
    finally:
        del exc


class FileHandler(StreamHandler):
    terminator = "\n"

    def __init__(self, filename, mode="a", encoding=None, delay=False, errors=None):
        self.baseFilename = os.path.abspath(os.fspath(filename))
        self.mode = mode
        self.encoding = encoding if "b" in mode else io.text_encoding(encoding)
        self.errors = errors
        self.delay = delay
        self._builtin_open = open

        # Subclasses can override _open, including during eager construction.
        stream = None
        if not delay:
            stream = self._open()
            if stream is None:
                stream = sys.stderr
        Handler.__init__(self)
        self.stream = stream

    def _open(self):
        return self._builtin_open(self.baseFilename, self.mode,
                                  encoding=self.encoding, errors=self.errors)

    def emit(self, record):
        if self.stream is None and (self.mode != "w" or not self._closed):
            self.stream = self._open()
        if self.stream:
            StreamHandler.emit(self, record)

    def close(self):
        with self.lock:
            try:
                if self.stream:
                    try:
                        self.flush()
                    finally:
                        stream = self.stream
                        self.stream = None
                        if hasattr(stream, "close"):
                            stream.close()
            finally:
                StreamHandler.close(self)

    def __repr__(self):
        return "<%s %s (%s)>" % (type(self).__name__, self.baseFilename,
                                  getLevelName(self.level))


class Filterer(object):
    """
    A base class for loggers and handlers which allows them to share
    common code.
    """
    def __init__(self):
        """
        Initialize the list of filters to be an empty list.
        """
        self.filters = []

    def addFilter(self, filter):
        """
        Add the specified filter to this handler.
        """
        if not (filter in self.filters):
            self.filters.append(filter)

    def removeFilter(self, filter):
        """
        Remove the specified filter from this handler.
        """
        if filter in self.filters:
            self.filters.remove(filter)

    def filter(self, record):
        """
        Determine if a record is loggable by consulting all the filters.

        The default is to allow the record to be logged; any filter can veto
        this by returning a false value.
        If a filter attached to a handler returns a log record instance,
        then that instance is used in place of the original log record in
        any further processing of the event by that handler.
        If a filter returns any other true value, the original log record
        is used in any further processing of the event by that handler.

        If none of the filters return false values, this method returns
        a log record.
        If any of the filters return a false value, this method returns
        a false value.

        .. versionchanged:: 3.2

           Allow filters to be just callables.

        .. versionchanged:: 3.12
           Allow filters to return a LogRecord instead of
           modifying it in place.
        """
        for f in self.filters:
            if hasattr(f, 'filter'):
                result = f.filter(record)
            else:
                result = f(record) # assume callable - will raise if not
            if not result:
                return False
            if isinstance(result, LogRecord):
                record = result
        return record


class Logger(Filterer):
    def __init__(self, name, level=NOTSET):
        Filterer.__init__(self)
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

    def findCaller(self, stack_info=False, stacklevel=1):
        """
        Find the stack frame of the caller so that we can note the source
        file name, line number and function name.
        """
        f = currentframe()
        #On some versions of IronPython, currentframe() returns None if
        #IronPython isn't run with -X:Frames.
        if f is None:
            return "(unknown file)", 0, "(unknown function)", None
        while stacklevel > 0:
            next_f = f.f_back
            if next_f is None:
                ## We've got options here.
                ## If we want to use the last (deepest) frame:
                break
                ## If we want to mimic the warnings module:
                #return ("sys", 1, "(unknown function)", None)
                ## If we want to be pedantic:
                #raise ValueError("call stack is not deep enough")
            f = next_f
            if not _is_internal_frame(f):
                stacklevel -= 1
        co = f.f_code
        sinfo = None
        if stack_info:
            with io.StringIO() as sio:
                sio.write("Stack (most recent call last):\n")
                traceback.print_stack(f, file=sio)
                sinfo = sio.getvalue()
                if sinfo[-1] == '\n':
                    sinfo = sinfo[:-1]
        return co.co_filename, f.f_lineno, co.co_name, sinfo

    def makeRecord(self, name, level, fn, lno, msg, args, exc_info,
                   func=None, extra=None, sinfo=None):
        """
        A factory method which can be overridden in subclasses to create
        specialized LogRecords.
        """
        rv = _logRecordFactory(name, level, fn, lno, msg, args, exc_info, func,
                             sinfo)
        if extra is not None:
            for key in extra:
                if (key in ["message", "asctime"]) or (key in rv.__dict__):
                    raise KeyError("Attempt to overwrite %r in LogRecord" % key)
                rv.__dict__[key] = extra[key]
        return rv

    def log(self, level, msg, *args, **kwargs):
        """
        Log 'msg % args' with the integer severity 'level'.

        To pass exception information, use the keyword argument exc_info with
        a true value, e.g.

        logger.log(level, "We have a %s", "mysterious problem", exc_info=True)
        """
        if not isinstance(level, int):
            if raiseExceptions:
                raise TypeError("level must be an integer")
            else:
                return
        if self.isEnabledFor(level):
            self._log(level, msg, args, **kwargs)

    def _log(self, level, msg, args, exc_info=None, extra=None, stack_info=False,
             stacklevel=1):
        """
        Low-level logging routine which creates a LogRecord and then calls
        all the handlers of this logger to handle the record.
        """
        sinfo = None
        if _srcfile:
            #IronPython doesn't track Python frames, so findCaller raises an
            #exception on some versions of IronPython. We trap it here so that
            #IronPython can use logging.
            try:
                fn, lno, func, sinfo = self.findCaller(stack_info, stacklevel)
            except ValueError: # pragma: no cover
                fn, lno, func = "(unknown file)", 0, "(unknown function)"
        else: # pragma: no cover
            fn, lno, func = "(unknown file)", 0, "(unknown function)"
        if exc_info:
            if isinstance(exc_info, BaseException):
                exc_info = (type(exc_info), exc_info, exc_info.__traceback__)
            elif not isinstance(exc_info, tuple):
                exc_info = sys.exc_info()
        record = self.makeRecord(self.name, level, fn, lno, msg, args,
                                 exc_info, func, extra, sinfo)
        self.handle(record)

    def handle(self, record):
        """
        Call the handlers for the specified record.

        This method is used for unpickled records received from a socket, as
        well as those created locally. Logger-level filtering is applied.
        """
        if self.disabled:
            return
        maybe_record = self.filter(record)
        if not maybe_record:
            return
        if isinstance(maybe_record, LogRecord):
            record = maybe_record
        self.callHandlers(record)

    def callHandlers(self, record):
        current = self
        found = False
        while current is not None:
            for handler in current.handlers:
                found = True
                if record.levelno >= handler.level:
                    handler.handle(record)
            if not current.propagate:
                break
            current = current.parent
        if not found and record.levelno >= WARNING:
            sys.stderr.write(_default_formatter.format(record) + "\n")
            sys.stderr.flush()

    def info(self, msg, *args, **kwargs):
        """
        Log 'msg % args' with severity 'INFO'.

        To pass exception information, use the keyword argument exc_info with
        a true value, e.g.

        logger.info("Houston, we have a %s", "notable problem", exc_info=True)
        """
        if self.isEnabledFor(INFO):
            self._log(INFO, msg, args, **kwargs)

    def debug(self, msg, *args, **kwargs):
        """
        Log 'msg % args' with severity 'DEBUG'.

        To pass exception information, use the keyword argument exc_info with
        a true value, e.g.

        logger.debug("Houston, we have a %s", "thorny problem", exc_info=True)
        """
        if self.isEnabledFor(DEBUG):
            self._log(DEBUG, msg, args, **kwargs)

    def warning(self, msg, *args, **kwargs):
        """
        Log 'msg % args' with severity 'WARNING'.

        To pass exception information, use the keyword argument exc_info with
        a true value, e.g.

        logger.warning("Houston, we have a %s", "bit of a problem", exc_info=True)
        """
        if self.isEnabledFor(WARNING):
            self._log(WARNING, msg, args, **kwargs)

    def error(self, msg, *args, **kwargs):
        """
        Log 'msg % args' with severity 'ERROR'.

        To pass exception information, use the keyword argument exc_info with
        a true value, e.g.

        logger.error("Houston, we have a %s", "major problem", exc_info=True)
        """
        if self.isEnabledFor(ERROR):
            self._log(ERROR, msg, args, **kwargs)

    def exception(self, msg, *args, exc_info=True, **kwargs):
        """
        Convenience method for logging an ERROR with exception information.
        """
        self.error(msg, *args, exc_info=exc_info, **kwargs)


    def critical(self, msg, *args, **kwargs):
        """
        Log 'msg % args' with severity 'CRITICAL'.

        To pass exception information, use the keyword argument exc_info with
        a true value, e.g.

        logger.critical("Houston, we have a %s", "major disaster", exc_info=True)
        """
        if self.isEnabledFor(CRITICAL):
            self._log(CRITICAL, msg, args, **kwargs)


def getLogger(name=None):
    return _native.get(name)


root = getLogger()
