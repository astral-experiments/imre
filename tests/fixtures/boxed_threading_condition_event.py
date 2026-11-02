import threading
import warnings
from threading import Condition, Event


def condition_wait():
    cond = Condition()
    with cond:
        cond.acquire()
        try:
            assert not cond.wait(0.001)
            assert cond._lock._recursion_count() == 2
            assert not cond._waiters
            assert cond.wait_for(lambda: 42, 0) == 42
        finally:
            cond.release()
    for method in (cond.wait, cond.notify):
        try:
            method()
        except RuntimeError:
            pass
        else:
            raise AssertionError("operation on an unacquired Condition succeeded")


def native_contention():
    cond = Condition(threading.Lock())
    cond.acquire()
    worker = threading.Thread(target=cond.acquire)
    try:
        worker.start()
        assert worker.is_alive()
    finally:
        cond.release()
    worker.join(5)
    assert not worker.is_alive() and cond.locked()
    cond.release()


def event_pulse():
    event = Event()

    class Pulse(float):
        def __gt__(self, other):
            # Event.wait enrolled its waiter and released the underlying lock.
            event.set()
            event.clear()
            return True

    assert event.wait(Pulse(0.1)) and not event.is_set()
    assert not event.wait(0)
    event.set()
    assert event.wait(object())
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert event.isSet()
        with event._cond:
            event._cond.notifyAll()
    assert len(caught) == 2
    assert all(w.category is DeprecationWarning and w.filename == __file__ for w in caught)


assert Condition is threading.Condition and Event is threading.Event
assert "Condition" in threading.__all__ and "Event" in threading.__all__
condition_wait()
native_contention()
event_pulse()
