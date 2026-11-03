import threading
from threading import Timer


def direct_run():
    seen = []

    def record(value, *, suffix):
        seen.append(value + suffix)

    timer = Timer(3, seen.append)
    other = Timer(0, seen.append)
    assert isinstance(timer, threading.Thread)
    assert timer.args is not other.args and timer.kwargs is not other.kwargs

    class ReplaceCallback(threading.Event):
        def wait(self, timeout=None):
            assert timeout == 3
            timer.function = record
            timer.args = ["new"]
            timer.kwargs = {"suffix": "!"}
            return False

    timer.finished = ReplaceCallback()
    timer.run()
    assert timer.finished.is_set() and seen == ["new!"]

    other.cancel()
    other.run()
    assert other.finished.is_set() and seen == ["new!"]


def callback_error():
    def fail():
        raise ValueError("timer callback")

    timer = Timer(0, fail)
    try:
        timer.run()
    except ValueError as error:
        assert str(error) == "timer callback"
    else:
        raise AssertionError("Timer.run swallowed the callback error")
    assert not timer.finished.is_set()


assert "Timer" in threading.__all__
direct_run()
callback_error()
