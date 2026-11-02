import logging


def current():
    calls = []

    class Sink(logging.Handler):
        def emit(self, record):
            calls.append(record)

    class Replace:
        def filter(self, record):
            assert record is original
            return replacement

    original = logging.makeLogRecord({"msg": "original"})
    replacement = logging.makeLogRecord({"msg": "replacement"})
    handler = Sink()
    assert logging.Handler.__bases__ == (logging.Filterer,)
    assert logging.Handler.filter is logging.Logger.filter
    assert handler.handle(original) is original and calls == [original]
    replace = Replace()

    def observe(record):
        assert record is replacement
        return True

    handler.addFilter(filter=replace)
    handler.addFilter(replace)
    handler.addFilter(observe)
    assert handler.filters == [replace, observe]
    assert handler.handle(record=original) is replacement
    assert calls == [original, replacement]

    def reject(record):
        return 0

    handler.addFilter(reject)
    assert handler.handle(original) is False and len(calls) == 2
    handler.removeFilter(filter=reject)
    handler.removeFilter(reject)
    handler.removeFilter(observe)
    handler.removeFilter(replace)
    assert handler.handle(None) is None and len(calls) == 2

    handler.addFilter(None)
    try:
        handler.handle(original)
    except TypeError:
        pass
    else:
        raise AssertionError("an invalid filter did not raise")
    assert len(calls) == 2

    class Override(Sink):
        def filter(self, record):
            return "emit"

    assert Override().handle(original) == "emit" and calls[-1] is original
    assert logging.NullHandler().handle(original) is None
    print("handler filters, replacement, veto and override")
