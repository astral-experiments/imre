import io
import logging
import time


class ExceptionFormatter(logging.Formatter):
    def formatException(self, exc_info):
        return str(exc_info[1])

    def formatStack(self, stack_info):
        return "stack: " + stack_info


def current():
    output = io.StringIO()
    handler = logging.StreamHandler(output)
    logger = logging.Logger("worker", logging.INFO)
    logger.addHandler(handler)
    try:
        handler.setFormatter(logging.Formatter("%(levelname)s %(team)s %(message)s"))
        logger.info("finished %d", 3, extra={"team": "ops"})
        assert output.getvalue() == "INFO ops finished 3\n"
    finally:
        handler.close()

    record = logging.makeLogRecord({"msg": "job", "created": 0.0, "msecs": 123.0, "lineno": 7})
    fmt = logging.Formatter("{asctime} {lineno:03d} {message!r}", style="{")
    fmt.converter = time.gmtime
    assert fmt.usesTime()
    assert fmt.format(record) == "1970-01-01 00:00:00,123 007 'job'"

    err = ValueError("boom")
    record.exc_info = (type(err), err, err.__traceback__)
    record.stack_info = "worker"
    assert ExceptionFormatter().format(record) == "job\nboom\nstack: worker"
    assert record.exc_text == "boom"
    assert logging.Formatter().format(record) == "job\nboom\nworker"
