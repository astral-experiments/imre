import logging
import sys


class Stream:
    def __init__(self):
        self.writes = []
        self.flushes = 0

    def write(self, message):
        self.writes.append(message)
        return len(message)

    def flush(self):
        self.flushes += 1


def current():
    check_root_records()
    assert logging.StreamHandler().stream is sys.stderr
    stream = Stream()
    child = logging.getLogger("access.child")
    parent = logging.getLogger("access")
    assert child.parent is parent
    assert parent.parent is logging.getLogger()
    assert logging.getLogger("access.child") is child

    independent = logging.Logger("access.child")
    assert independent is not child and independent.parent is None
    assert independent.handlers is not child.handlers
    parent.setLevel("INFO")
    assert child.level == logging.NOTSET and child.getEffectiveLevel() == logging.INFO

    handler = logging.StreamHandler(stream)
    assert repr(handler) == "<StreamHandler (NOTSET)>"
    stream.name = 2
    assert repr(handler) == "<StreamHandler 2 (NOTSET)>"
    handler.name = "access-stream"
    assert logging.getHandlerByName("access-stream") is handler
    handler.name = "renamed-stream"
    assert logging.getHandlerByName("access-stream") is None
    names = logging.getHandlerNames()
    parent.addHandler(handler)
    parent.addHandler(handler)
    assert len(parent.handlers) == 1
    assert isinstance(handler, logging.Handler) and handler.stream is stream
    child.info("GET %s %d", "/health", 200)
    child.disabled = True
    child.error("disabled")
    child.disabled = False
    child.debug("below level")
    handler.setLevel(logging.ERROR)
    child.warning("below handler")
    child.error("GET %s %d", "/missing", 404)
    assert stream.writes == ["GET /health 200\n", "GET /missing 404\n"]
    assert stream.flushes == 2
    handler.close()
    assert names == frozenset({"renamed-stream"})
    assert not logging.getHandlerNames()
    print(stream.writes, stream.flushes)

    silent = logging.NullHandler()
    assert isinstance(silent, logging.Handler) and silent.lock is None
    assert silent.handle(None) is None
    assert silent.emit(None) is None
    silent.close()

    original = logging.getLogRecordFactory()
    recorded = []

    class CustomRecord(logging.LogRecord):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            recorded.append(self)

    try:
        logging.setLogRecordFactory(CustomRecord)
        payload = {"name": "access", "msg": "request %d", "args": (42,)}
        record = logging.makeLogRecord(payload)
        assert type(record) is CustomRecord and record.getMessage() == "request 42"
        assert record.levelname == "Level None" and record.taskName is None
        child.info("issued %s", "GET")
        issued = recorded[-1]
        assert issued.getMessage() == "issued GET"
        assert issued.funcName == "current" and issued.lineno > 0
        assert issued.pathname == current.__code__.co_filename

        def helper():
            child.info("context", extra={"user_id": "42"}, stacklevel=2)

        class OverrideLog(logging.Logger):
            def log(self, *args, **kwargs):
                raise AssertionError("convenience method called overridden log")

        OverrideLog("override", logging.INFO).info("normal info path")
        assert recorded[-1].getMessage() == "normal info path"
        helper()
        assert recorded[-1].funcName == "current" and recorded[-1].user_id == "42"
        record = child.makeRecord("access", logging.INFO, "app.py", 11, "ok", (), None,
                                  extra={"request_id": 42})
        assert record.request_id == 42 and record.module == "app"
        try:
            child.makeRecord("access", logging.INFO, "app.py", 11, "ok", (), None,
                             extra={"message": "override"})
        except KeyError:
            pass
        else:
            raise AssertionError("accepted reserved field")
    finally:
        logging.setLogRecordFactory(original)


def check_root_records():
    root = logging.root
    stream = Stream()
    handler = logging.StreamHandler(stream)
    logging.basicConfig(handlers=[handler], format="%(levelname)s %(message)s")
    try:
        logging.warning("less is %(less)s", {"less": "more"})
        assert stream.writes == ["WARNING less is more\n"]
        try:
            raise ValueError("root failure")
        except ValueError:
            logging.exception("failed")
        assert stream.writes[-1].startswith("ERROR failed\nTraceback (most recent call last):")
        assert stream.writes[-1].endswith("ValueError: root failure\n")
        logging.basicConfig(handlers=[])
        assert root.handlers == [handler]
    finally:
        root.removeHandler(handler)
        handler.close()
