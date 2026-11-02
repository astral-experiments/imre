import gc
import io
import logging
import weakref


def current():
    assert logging.Handler.__init__.__name__ == "__init__"
    assert logging.NullHandler.__init__.__qualname__ == "Handler.__init__"
    assert not hasattr(logging, "_handler_init")
    registry = logging._handlerList
    fallback = logging._defaultLastResort
    assert logging.lastResort is fallback and type(fallback) is logging._StderrHandler
    assert registry[0]() is fallback and registry[0].__callback__ is logging._removeHandlerRef
    assert fallback.level == logging.WARNING and fallback.terminator == "\n"
    saved = registry[:]
    previous = logging.raiseExceptions
    events = []

    class Recorder(logging.Handler):
        def __init__(self, label):
            self.label = label
            self.close_error = None
            super().__init__()

        def createLock(self):
            assert registry[-1]() is self
            assert not hasattr(self, "lock")
            super().createLock()

        def acquire(self):
            events.append((self.label, "acquire"))
            return super().acquire()

        def flush(self):
            events.append((self.label, "flush"))

        def close(self):
            events.append((self.label, "close"))
            if self.close_error:
                raise self.close_error
            super().close()

        def release(self):
            events.append((self.label, "release"))
            return super().release()

    try:
        output = io.StringIO()
        fallback_handler = logging.StreamHandler(output)
        fallback_handler.setLevel(logging.ERROR)
        logging.lastResort = fallback_handler
        probe = logging.Logger("unhandled")
        probe.warning("quiet")
        probe.error("reported")
        assert output.getvalue() == "reported\n"
        registry[:] = []
        handler = logging.NullHandler()
        handler.cycle = handler
        dead = registry[-1]
        assert dead() is handler
        assert dead.__callback__ is logging._removeHandlerRef
        del handler
        gc.collect()
        assert dead() is None and not registry
        logging._removeHandlerRef(dead)

        first = Recorder("first")
        second = Recorder("second")
        first.flushOnClose = False
        first_ref, second_ref = registry
        first.close()
        assert first_ref() is first and second_ref() is second
        assert registry == [first_ref, second_ref]
        events.clear()
        logging.shutdown()
        assert events == [("second", "acquire"), ("second", "flush"),
                          ("second", "close"), ("second", "release"),
                          ("first", "acquire"), ("first", "close"),
                          ("first", "release")]

        # Traversal keeps its snapshot even when callbacks change the live list.
        class Mutator(Recorder):
            def flush(self):
                registry.clear()
        mutator = Mutator("mutator")
        events.clear()
        logging.shutdown()
        assert not registry and events[-1] == ("first", "release")

        refs = [weakref.ref(first), dead, weakref.ref(second)]
        for error in (OSError("closed"), ValueError("closed")):
            logging.raiseExceptions = True
            second.close_error = error
            events.clear()
            logging.shutdown(handlerList=refs)
            assert events[-1] == ("first", "release")
        second.close_error = RuntimeError("broken")
        events.clear()
        try:
            logging.shutdown(refs)
        except RuntimeError as exc:
            assert exc is second.close_error
        else:
            raise AssertionError("close error was lost")
        assert events[-1] == ("second", "release")
        logging.raiseExceptions = False
        events.clear()
        logging.shutdown(refs)
        assert events[-1] == ("first", "release")
        second.close_error = None
    finally:
        logging.lastResort = fallback
        logging.raiseExceptions = previous
        registry[:] = saved
    print("weak handler lifecycle and shutdown")
