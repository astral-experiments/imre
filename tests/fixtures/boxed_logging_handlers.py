import io
import logging
import logging.handlers
import queue


def current():
    output = io.StringIO()
    stream = logging.StreamHandler(output)
    memory = logging.handlers.MemoryHandler(3, logging.WARNING, stream)
    record = logging.LogRecord("app", logging.INFO, "app.py", 1, "hello", (), None)
    memory.handle(record)
    assert output.getvalue() == ""
    assert memory.buffer == [record]
    memory.flush()
    assert output.getvalue() == "hello\n"
    assert memory.buffer == []
    memory.close()
    assert memory.target is None

    records = queue.Queue()
    handler = logging.handlers.QueueHandler(records)
    handler.handle(record)
    received = records.get_nowait()
    assert received is not record
    assert received.msg == "hello"
    assert received.name == "app"
    listener = logging.handlers.QueueListener(records, stream)
    listener.handle(received)
    assert output.getvalue() == "hello\nhello\n"

    class RedirectOnFlush(logging.StreamHandler):
        def flush(self):
            self.stream = None

    redirected = RedirectOnFlush(output)
    replacement = io.StringIO()
    assert redirected.setStream(replacement) is output
    assert redirected.stream is replacement
    assert redirected.setStream(replacement) is None
    redirected.close()

    handler.close()
    stream.close()
