import io
import logging
import os
import sys


def current():
    path = __file__ + ".log"
    h = logging.FileHandler(path, "w", encoding="ascii")
    original_stderr = sys.stderr
    original_raise = logging.raiseExceptions
    original_handlers = logging.root.handlers
    original_level = logging.root.level
    record = logging.makeLogRecord({"msg": "snow %s", "args": ("☃",)})
    try:
        stream = h.stream
        sys.stderr = io.StringIO()
        logging.raiseExceptions = True
        h.handle(record)
        diagnostic = sys.stderr.getvalue()
        assert diagnostic.startswith("--- Logging error ---\nTraceback ")
        assert "UnicodeEncodeError:" in diagnostic and "Call stack:\n" in diagnostic
        assert "Message: 'snow %s'\nArguments: ('☃',)\n" in diagnostic

        logging.raiseExceptions = False
        h.handle(record)
        assert sys.stderr.getvalue() == diagnostic

        caught = []
        def handle_error(actual):
            caught.append((actual, sys.exception()))
        h.handleError = handle_error
        h.handle(record)
        [(actual, error)] = caught
        assert actual is record and isinstance(error, UnicodeEncodeError)

        logging.root.handlers = []
        logging.root.setLevel(logging.WARNING)
        logging.debug("below threshold")
        assert len(logging.root.handlers) == 1
        assert logging.root.handlers[0].stream is sys.stderr
        assert sys.stderr.getvalue() == diagnostic

        logging.basicConfig(force=True, handlers=[h], level=logging.DEBUG)
        logging.debug("snow %s", "☃", extra={"request_id": 42})
        assert len(caught) == 2
        actual, error = caught[1]
        assert actual.name == "root" and actual.levelno == logging.DEBUG
        assert actual.getMessage() == "snow ☃" and actual.request_id == 42
        assert isinstance(error, UnicodeEncodeError)
        h.close()
        assert stream.closed
        with open(path, "rb") as f:
            assert f.read() == b""
    finally:
        logging.root.handlers = original_handlers
        logging.root.setLevel(original_level)
        sys.stderr = original_stderr
        logging.raiseExceptions = original_raise
        h.close()
        os.unlink(path)
