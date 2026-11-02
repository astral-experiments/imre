import _thread
import contextvars
import sys
import threading
import time
from tempfile import NamedTemporaryFile


class CheckAtClose:
    def __init__(self, file, blocked, finished):
        self.file = file
        self.blocked = blocked
        self.finished = finished
        self.unstarted = threading.Thread()
        self.explicit = threading.Thread(context=contextvars.Context())

    def close(self, start=_thread.start_new_thread, is_finalizing=sys.is_finalizing):
        self.file.close()
        assert is_finalizing()
        for operation, args in (
            (start, (print, ())),
            (self.unstarted.start, ()),
            (self.explicit.start, ()),
            (self.blocked.join, (0,)),
        ):
            try:
                operation(*args)
            except PythonFinalizationError as error:
                print(str(error), flush=True)
            else:
                raise AssertionError("thread operation succeeded at shutdown")
        self.finished.join()
        print("joined completed worker", flush=True)


gate = _thread.allocate_lock()
gate.acquire()
blocked = threading.Thread(target=gate.acquire, daemon=True)
blocked.start()
finished = threading.Thread(daemon=True)
finished.start()
while finished.is_alive():
    time.sleep(0.001)

# NamedTemporaryFile's closer is finalized after ordinary exit callbacks.
kept = NamedTemporaryFile()
kept._closer.file = CheckAtClose(kept.file, blocked, finished)
