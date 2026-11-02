import errno
import gc
import subprocess
import sys
import weakref


class Marker:
    pass


class InvalidSignal:
    def __index__(self):
        raise AssertionError("unexpected signal conversion")


def current():
    child = subprocess.Popen(["/bin/cat"], stdin=subprocess.PIPE)
    try:
        for value in (False, 2**90, -(2**90), Marker()):
            child.returncode = value
            assert child.returncode is value and child.poll() is value
            assert child.wait(0) is value
            child.send_signal(InvalidSignal())

        marker = Marker()
        ref = weakref.ref(marker)
        child.returncode = marker
        del marker
        gc.collect()
        assert ref() is not None and child.returncode is ref()
        child.returncode = None
        gc.collect()
        assert ref() is None

        def change_status():
            child.returncode = False

        child.poll = change_status
        child.send_signal(InvalidSignal())
        assert child.returncode is False

        problem = ValueError("poll failed")

        def fail_poll():
            raise problem

        child.poll = fail_poll
        try:
            child.send_signal(0)
        except ValueError as exc:
            assert exc is problem
        else:
            raise AssertionError("poll exception lost")

        child.returncode = None
        # The override's return value is ignored; the child still owns its PID.
        child.poll = lambda: 23
        child.send_signal(0)
        if sys.maxsize > 2**32:
            child.send_signal(2**32)  # Narrows to the harmless POSIX signal zero.
        try:
            child.send_signal(sys.maxsize + 1)
        except OverflowError:
            pass
        else:
            raise AssertionError("signal outside Py_ssize_t was accepted")
        try:
            child.send_signal(-1)
        except OSError as exc:
            assert exc.errno == errno.EINVAL
        else:
            raise AssertionError("expected kernel EINVAL for live child")
        del child.poll
        assert child.poll() is None

        class SetDuringIndex:
            def __index__(self):
                child.returncode = False
                return -1

        try:
            child.send_signal(SetDuringIndex())
        except OSError as exc:
            assert exc.errno == errno.EINVAL and child.returncode is False
        else:
            raise AssertionError("signal conversion suppressed a live kernel error")

        class MissingDuringIndex:
            def __index__(self):
                raise ProcessLookupError()

        child.returncode = None
        child.send_signal(MissingDuringIndex())

        class ReapDuringIndex:
            def __index__(self):
                child.stdin.close()
                assert child.wait() == 0
                return 0

        child.send_signal(ReapDuringIndex())
        assert child.returncode == 0
    finally:
        child.returncode = None
        child.stdin.close()
        assert child.wait() == 0

    # Public state can be cleared after the kernel has released the PID.
    child = subprocess.Popen(["/bin/sh", "-c", "exit 1"])
    status = child.wait()
    assert status == 1 and child.poll() is status and child.returncode is status
    child.returncode = None
    child.poll = lambda: None
    child.send_signal(0)
    assert child.returncode is None
    del child.poll
    child.send_signal(InvalidSignal())
    assert child.returncode == 0
