import io
import logging
import sys


def current():
    events = []
    late = io.StringIO()
    early = io.StringIO()

    class Selected(logging.FileHandler):
        def _open(self):
            events.append("open")
            return selected

        def createLock(self):
            events.append("lock")
            super().createLock()
            sys.stderr = late

        def __setattr__(self, name, value):
            if name == "stream":
                events.append(value)
            super().__setattr__(name, value)

    with open(__file__, encoding="utf-8") as explicit:
        cases = ((True, None, None), (False, explicit, explicit), (False, None, late))
        for delay, selected, expected in cases:
            events.clear()
            saved = sys.stderr
            try:
                sys.stderr = early
                handler = Selected(__file__, "r", encoding="utf-8", delay=delay)
            finally:
                sys.stderr = saved
            try:
                assert handler.stream is expected
                assert events == (["lock", expected] if delay else ["open", "lock", expected])
            finally:
                handler.close()
