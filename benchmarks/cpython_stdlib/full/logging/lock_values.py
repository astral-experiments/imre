"""Exercise handler lock ownership and context cleanup without a CPython fixture."""
import logging


def replace_in_context(handler: logging.Handler) -> int:
    old = handler.lock
    if old is None:
        return -1
    print(old is handler.lock)
    with old as acquired:
        print(acquired)
        with old:
            print(old._recursion_count())
        handler.createLock()
        print(old is handler.lock)
        return old._recursion_count()


def enter_optional(handler: logging.Handler) -> None:
    with handler.lock:
        print("entered")


def exit_argument() -> str:
    print("exit argument")
    return "unused"


handler = logging.Handler()
old = handler.lock
if old is not None:
    print(old.locked())
    print(handler.acquire() is None)
    print(old._is_owned())
    print(old.acquire(False))
    print(old._recursion_count())
    print(handler.release() is None)
    print(old._recursion_count())
    handler.release()
    print(old.locked())
    print(replace_in_context(handler))
    print(old.locked())
lock = handler.lock
if lock is not None:
    try:
        with handler.lock:
            print(lock._is_owned())
            raise ValueError("leave lock")
    except ValueError as error:
        print(str(error))
    print(lock.locked())
    print(lock.acquire(timeout=0.0))
    handler._at_fork_reinit()
    print(lock.locked())
    try:
        lock.release()
    except RuntimeError as error:
        print(str(error))
    try:
        lock.acquire(False, 0.0)
    except ValueError as error:
        print(str(error))
    try:
        lock.acquire(timeout=-0.1)
    except ValueError as error:
        print(str(error))
    try:
        lock.acquire(timeout=1000000000000.0)
    except OverflowError as error:
        print(str(error))
if lock is not None:
    try:
        lock.acquire(timeout=1000000000000)
    except OverflowError as error:
        print(str(error))
    print(lock.acquire(timeout=0))
    lock.release()
if lock is not None:
    print(lock.__enter__(False))
    print(lock.__exit__(None, exit_argument(), None) is None)
    print(lock._is_owned())
    print(lock.acquire(0))
    lock.__exit__()
null = logging.NullHandler()
print(null.lock is None)
print(null.acquire() is None)
print(null.release() is None)
print(null._at_fork_reinit() is None)
handler.lock = None
try:
    enter_optional(handler)
except TypeError as error:
    print(str(error))
handler.close()
