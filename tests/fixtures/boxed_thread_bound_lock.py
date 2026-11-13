import _thread
import threading
import time


class Receiver:
    def __init__(self, gate):
        self.gate = gate

    def task(self):
        self.gate.acquire()


def replacement(self):
    raise AssertionError("a bound target was looked up again")


def check():
    first = _thread.allocate_lock()
    second = _thread.allocate_lock()
    first.acquire()
    second.acquire()
    one = Receiver(first)
    two = Receiver(second)
    target = one.task
    thread = threading.Thread(target=two.task)
    original = Receiver.task
    Receiver.task = replacement
    count = _thread._count()
    try:
        ident = _thread.start_new_thread(target, ())
        thread.start()
        assert ident != thread.ident and ident != _thread.get_ident()
        thread.join(0.01)
        assert thread.is_alive()
    finally:
        Receiver.task = original
        first.release()
        second.release()
        if thread.ident is not None:
            thread.join(3)
    deadline = time.monotonic() + 3
    while _thread._count() != count or not first.locked():
        assert time.monotonic() < deadline
        time.sleep(0.001)
    assert second.locked() and not thread.is_alive()


check()


def acquire(self, alias, *, done):
    self.acquire(False)
    if alias.locked():
        done.acquire(False)


gate = _thread.allocate_lock()
done = _thread.allocate_lock()
worker = threading.Thread(target=acquire.__get__(gate), args=(gate,), kwargs={"done": done})
worker.start()
worker.join(3)
assert not worker.is_alive()
assert gate.locked() and done.locked()
