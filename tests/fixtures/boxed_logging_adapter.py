import logging


class Recorder(logging.Handler):
    def __init__(self):
        super().__init__()
        self.records = []

    def emit(self, record):
        self.records.append(record)


def current():
    parent = logging.getLogger("request")
    parent.propagate = False
    logger = logging.getLogger("request.worker")
    logger.setLevel(logging.INFO)
    handler = Recorder()
    inner = logging.LoggerAdapter(logger, {"user": "inner", "team": "api"}, merge_extra=True)
    outer = logging.LoggerAdapter(inner, {"user": "outer"}, merge_extra=True)
    assert not outer.hasHandlers()
    parent.addHandler(handler)
    try:
        assert outer.hasHandlers()
        logger.propagate = False
        assert not outer.hasHandlers()
        logger.propagate = True
        try:
            raise ValueError("missing")
        except ValueError as error:
            outer.exception("job %d", 3, extra={"user": "call"})
            record = handler.records[0]
            assert record.getMessage() == "job 3" and record.funcName == "current"
            assert record.user == "call" and record.team == "api"
            assert record.exc_info[1] is error
            assert record.exc_info[2] is error.__traceback__

        class DisabledAdapter(logging.LoggerAdapter):
            def process(self, msg, kwargs):
                raise AssertionError("processed disabled event")

        DisabledAdapter(logger).debug("disabled")
        assert len(handler.records) == 1
    finally:
        parent.removeHandler(handler)
        handler.close()
