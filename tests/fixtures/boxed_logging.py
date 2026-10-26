import logging


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
    print(stream.writes, stream.flushes)
