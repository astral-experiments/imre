import os
import sys


def current():
    read_fd, write_fd = os.pipe()
    events = []

    def first_before():
        events.append("a")

    def second_before():
        events.append("b")
        os.register_at_fork(after_in_child=lambda: events.append("d"))

    def on_audit(event, args):
        if event == "os.fork":
            assert args == ()
            events.append("audit")

    os.register_at_fork(before=first_before, after_in_child=lambda: events.append("c"))
    os.register_at_fork(before=second_before, after_in_parent=lambda: events.append("p"))
    sys.addaudithook(on_audit)
    try:
        pid = os.fork()
        if pid == 0:
            try:
                os.close(read_fd)
                os.write(write_fd, ",".join(events).encode())
            finally:
                os._exit(37)
        try:
            os.close(write_fd)
            write_fd = -1
            assert os.read(read_fd, 64) == b"audit,b,a,c,d"
        finally:
            waited, status = os.waitpid(pid, 0)
        assert waited == pid and os.waitstatus_to_exitcode(status) == 37
        assert events == ["audit", "b", "a", "p"]
    finally:
        os.close(read_fd)
        if write_fd != -1:
            os.close(write_fd)


def threaded_spawn():
    import random
    import threading
    import warnings

    # The automatic child reseed only touches private scalar state before immediate exec.
    random.gauss()

    # Keep a second native thread alive for the duration of the fork.
    gate = threading.Lock()
    gate.acquire()
    thread = threading.Thread(target=gate.acquire)
    thread.start()
    events = []
    os.register_at_fork(before=lambda: events.append("before"),
                        after_in_parent=lambda: events.append("parent"))
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("always", DeprecationWarning)
            def showwarning(message, category, *args, **kwargs):
                if issubclass(category, DeprecationWarning):
                    events.append("warning")
            warnings.showwarning = showwarning
            assert os.spawnl(os.P_WAIT, "/bin/true", "true") == 0
        assert events == ["before", "parent", "warning"]
    finally:
        gate.release()
        thread.join()
