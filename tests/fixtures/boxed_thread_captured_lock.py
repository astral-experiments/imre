import _thread
import threading


def check():
    original = _thread.allocate_lock()
    original.acquire()
    gate = _thread.allocate_lock()

    def acquire():
        gate.acquire()

    first = threading.Thread(target=acquire)
    gate = original
    first.start()
    try:
        first.join(0.01)
        assert first.is_alive()
        gate = _thread.allocate_lock()
    finally:
        original.release()
        first.join(3)
    assert not first.is_alive()
    assert original.locked() != gate.locked()

    # Source order differs from the ordering of the closure cells.
    z_started = _thread.allocate_lock()
    a_finish = _thread.allocate_lock()
    z_started.acquire()
    a_finish.acquire()

    def run():
        z_started.release()
        a_finish.acquire()

    second = threading.Thread(target=run)
    second.start()
    try:
        assert z_started.acquire(timeout=3)
        second.join(0.01)
        assert second.is_alive()
    finally:
        a_finish.release()
        second.join(3)
    assert not second.is_alive() and a_finish.locked()


check()
