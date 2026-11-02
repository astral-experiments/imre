import io
import logging
import os
import sys


def current():
    path = __file__ + ".log"
    h = logging.FileHandler(path, "w", encoding="ascii")
    original_stderr = sys.stderr
    original_raise = logging.raiseExceptions
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
            assert actual is record
            caught.append(sys.exception())
        h.handleError = handle_error
        h.handle(record)
        assert len(caught) == 1 and isinstance(caught[0], UnicodeEncodeError)
        h.close()
        assert stream.closed
        with open(path, "rb") as f:
            assert f.read() == b""
    finally:
        sys.stderr = original_stderr
        logging.raiseExceptions = original_raise
        h.close()
        os.unlink(path)
