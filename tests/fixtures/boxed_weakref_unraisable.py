import gc
import sys
import weakref


def current():
    class Target:
        pass

    class Callback:
        def __repr__(self):
            return "test callback"

        def __call__(self, ref):
            assert ref() is None
            raise ValueError("callback error")

    events = []
    hook = sys.unraisablehook
    sys.unraisablehook = events.append
    try:
        for cycle, prefix in ((False, "while"), (True, "on")):
            target = Target()
            if cycle:
                target.link = target
            ref = weakref.ref(target, Callback())
            del target
            gc.collect()
            assert ref() is None and len(events) == 1
            event = events.pop()
            assert event.exc_type is ValueError
            assert event.exc_value.args == ("callback error",)
            assert event.exc_traceback is not None
            assert event.object is None
            assert event.err_msg == "Exception ignored %s calling weakref callback test callback" % prefix
    finally:
        sys.unraisablehook = hook
