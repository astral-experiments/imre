from _thread import LockType
from threading import Lock


class MapLock:
    lock_class = Lock


def current():
    lock = MapLock().lock_class()
    print(type(lock) is Lock is LockType, isinstance(lock, Lock), Lock.__module__, Lock.__name__)
    print(lock.locked(), lock.acquire(False), lock.acquire(blocking=False), lock.acquire(timeout=0))
    print(Lock.release(lock), lock.locked())
    with lock as entered:
        print(entered, lock.locked(), lock.acquire(False))
    try:
        with lock:
            raise ValueError("body")
    except ValueError as error:
        print(str(error), lock.locked())
    for action in (
        lambda: lock.release(),
        lambda: lock.acquire(False, 1),
        lambda: lock.acquire(timeout=-0.5),
        lambda: lock.acquire(timeout=1e100),
        lambda: lock.acquire(timeout=10**50),
        lambda: lock.acquire(timeout=None),
        lambda: lock.acquire(False, blocking=False),
        lambda: Lock(1),
    ):
        try:
            action()
        except (RuntimeError, ValueError, TypeError, OverflowError) as error:
            print(type(error).__name__, str(error))
    print(lock.acquire(timeout=0.001), lock.acquire(timeout=0.001), lock.release())


if __name__ == "__main__":
    current()
