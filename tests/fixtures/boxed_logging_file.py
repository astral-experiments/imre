import io
import logging
import os


class TracedHandler(logging.FileHandler):
    terminator = "!"

    def _open(self):
        self.opens += 1
        return super()._open()


def current():
    class FramedStream(logging.StreamHandler):
        terminator = "\0"

    output = io.StringIO()
    framed = FramedStream(output)
    record = logging.makeLogRecord({"msg": "row"})
    framed.handle(record)
    FramedStream.terminator = "|"
    framed.handle(record)
    framed.terminator = "~"
    framed.handle(record)
    assert output.getvalue() == "row\0row|row~"

    path = __file__ + ".log"
    logger = logging.Logger("file")

    class Delay:
        calls = 0

        def __bool__(self):
            self.calls += 1
            return self.calls == 1

    delay = Delay()
    handler = TracedHandler(path, encoding="ascii", errors="backslashreplace", delay=delay)
    handler.opens = 0
    logger.addHandler(handler)
    try:
        assert handler.baseFilename == os.path.abspath(path)
        assert delay.calls == 1 and handler.stream is None and not os.path.exists(path)
        assert repr(handler) == f"<TracedHandler {handler.baseFilename} (NOTSET)>"
        logger.warning("café")
        first = handler.stream
        assert handler.opens == 1 and first.encoding == "ascii"
        assert first.mode == "a" and first.name == handler.baseFilename
        handler.close()
        assert first.closed and handler.stream is None and handler._closed
        logger.warning("again")
        assert handler.opens == 2 and handler.stream is not first
        with open(path, "rb") as stream:
            assert stream.read() == b"caf\\xe9!again!"
        try:
            logging.FileHandler(path, mode="x")
        except FileExistsError as exc:
            assert exc.filename == handler.baseFilename
        else:
            raise AssertionError("opened an existing log file exclusively")
    finally:
        logger.removeHandler(handler)
        handler.close()
        if os.path.exists(path):
            os.unlink(path)

    handler = logging.FileHandler(path, "w", encoding="utf-8")
    logger.addHandler(handler)
    try:
        logger.warning("first")
        handler.close()
        logger.warning("second")
        assert handler.stream is None
        with open(path, "rb") as stream:
            assert stream.read() == b"first\n"
    finally:
        logger.removeHandler(handler)
        handler.close()
        os.unlink(path)

    # A write-only stream may be flushed by emit() and by FileHandler.close().
    class Writer:
        def __init__(self, file):
            self.file = file

        def write(self, message):
            return self.file.write(message)

        def close(self):
            self.file.close()

    handler = logging.FileHandler(path, delay=True)
    try:
        with open(path, "w", encoding="utf-8") as raw:
            handler.stream = Writer(raw)
            handler.handle(logging.makeLogRecord({"msg": "saved"}))
            handler.close()
            assert raw.closed and handler.stream is None and handler._closed
        with open(path, "rb") as stream:
            assert stream.read() == b"saved\n"
    finally:
        if os.path.exists(path):
            os.unlink(path)

    class FalseStream:
        def __bool__(self):
            return False

        @property
        def flush(self):
            raise AssertionError("looked up flush on a false stream")

    logging.StreamHandler(FalseStream()).flush()

    error = AttributeError("from inside flush")

    class FailingStream:
        def flush(self):
            raise error

    handler = logging.StreamHandler(FailingStream())
    try:
        handler.flush()
    except AttributeError as caught:
        assert caught is error
    else:
        raise AssertionError("swallowed a flush failure")
    finally:
        handler.stream = None
