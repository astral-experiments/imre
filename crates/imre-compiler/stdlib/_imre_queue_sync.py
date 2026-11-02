"""CPython 3.14.5 threading algorithms for queue's primitive mutexes."""

from collections import deque
from threading import Lock
from time import monotonic


class Condition:
    def __init__(self, lock):
        self._lock = lock
        self.acquire = lock.acquire
        self.release = lock.release
        self.locked = lock.locked
        self._waiters = deque()

    def __enter__(self):
        return self._lock.__enter__()

    def __exit__(self, *args):
        return self._lock.__exit__(*args)

    def wait(self, timeout=None):
        if not self.locked():
            raise RuntimeError("cannot wait on un-acquired lock")
        waiter = Lock()
        waiter.acquire()
        self._waiters.append(waiter)
        self.release()
        acquired = False
        try:
            if timeout is None:
                acquired = waiter.acquire()
            elif timeout > 0:
                acquired = waiter.acquire(True, timeout)
            else:
                acquired = waiter.acquire(False)
            return acquired
        finally:
            self.acquire()
            if not acquired:
                try:
                    self._waiters.remove(waiter)
                except ValueError:
                    pass  # notify() already removed this waiter.

    def notify(self, n=1):
        if not self.locked():
            raise RuntimeError("cannot notify on un-acquired lock")
        while self._waiters and n > 0:
            waiter = self._waiters[0]
            try:
                waiter.release()
            except RuntimeError:
                # A previous notify() was interrupted after releasing this waiter.
                pass
            else:
                n -= 1
            try:
                self._waiters.remove(waiter)
            except ValueError:
                pass

    def notify_all(self):
        self.notify(len(self._waiters))


class Semaphore:
    def __init__(self, value):
        self._condition = Condition(Lock())
        self._value = value

    def acquire(self, blocking=True, timeout=None):
        if not blocking and timeout is not None:
            raise ValueError("can't specify timeout for non-blocking acquire")
        with self._condition:
            deadline = None
            while self._value == 0:
                if not blocking:
                    return False
                if timeout is not None:
                    if deadline is None:
                        deadline = monotonic() + timeout
                    else:
                        timeout = deadline - monotonic()
                        if timeout <= 0:
                            return False
                self._condition.wait(timeout)
            self._value -= 1
            return True

    def release(self):
        with self._condition:
            self._value += 1
            self._condition.notify()
