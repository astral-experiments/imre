"""Thread objects for the native object runtime."""

from _thread import TIMEOUT_MAX, LockType as Lock, _local as local, get_ident
from _thread import allocate_lock as _allocate_lock
import _thread
from _weakrefset import WeakSet
from itertools import count
import _imre_thread


__all__ = ["Thread", "Lock", "TIMEOUT_MAX", "local", "get_ident",
           "main_thread", "current_thread", "enumerate", "active_count"]
try:
    from _thread import get_native_id
except ImportError:
    _HAVE_THREAD_NATIVE_ID = False
else:
    _HAVE_THREAD_NATIVE_ID = True
    __all__.append("get_native_id")
try:
    _CRLock = _thread.RLock
except AttributeError:
    _CRLock = None
del _thread
_counter = count(1).__next__
_dangling = WeakSet()


class Thread:
    _initialized = False

    def __init__(self, group=None, target=None, name=None, args=(), kwargs=None,
                 *, daemon=None, context=None):
        assert group is None, "group argument must be None for now"
        if name:
            name = str(name)
        else:
            name = "Thread-%d" % _counter()
            if target is not None:
                try:
                    name += f" ({target.__name__})"
                except AttributeError:
                    pass
        self._name = name
        self._target = target
        self._args = args
        self._kwargs = {} if kwargs is None else kwargs
        if daemon is None:
            self._daemonic = current_thread().daemon
        else:
            # The main interpreter allows daemon threads; validate the flag's truth as CPython does.
            bool(daemon)
            self._daemonic = daemon
        self._context = context
        self._ident = None
        self._native_id = None
        self._handle = None
        self._initialized = True
        # Retain a weak entry for the lifetime of the object, including after join.
        _dangling.add(self)

    def run(self):
        try:
            if self._target is not None:
                self._target(*self._args, **self._kwargs)
        finally:
            del self._target, self._args, self._kwargs

    def start(self):
        if not self._initialized:
            raise RuntimeError("thread.__init__() not called")
        if self._ident is not None:
            raise RuntimeError("threads can only be started once")
        if self._context is not None:
            raise NotImplementedError("native workers cannot enter a Python context")
        # Truth conversion can run Python and replace the target or even start this thread.
        daemon = bool(self.daemon)
        if self._ident is not None:
            raise RuntimeError("threads can only be started once")
        self._handle, self._ident, self._native_id = _imre_thread.start(self, daemon)

    def join(self, timeout=None):
        if not self._initialized:
            raise RuntimeError("Thread.__init__() not called")
        if self._ident is None:
            raise RuntimeError("cannot join thread before it is started")
        if self is current_thread():
            raise RuntimeError("cannot join current thread")
        if timeout is not None:
            timeout = max(timeout, 0)
        _imre_thread.join(self._handle, timeout)

    def is_alive(self):
        assert self._initialized, "Thread.__init__() not called"
        return self._handle is not None and _imre_thread.alive(self._handle)

    def __repr__(self):
        assert self._initialized, "Thread.__init__() was not called"
        status = "initial"
        if self._ident is not None:
            status = "started"
            if self._handle is not None and not _imre_thread.alive(self._handle):
                status = "stopped"
        if self._daemonic:
            status += " daemon"
        if self._ident is not None:
            status += f" {self._ident}"
        return f"<{self.__class__.__name__}({self._name}, {status})>"

    @property
    def name(self):
        assert self._initialized, "Thread.__init__() not called"
        return self._name

    @name.setter
    def name(self, name):
        assert self._initialized, "Thread.__init__() not called"
        self._name = str(name)

    @property
    def ident(self):
        assert self._initialized, "Thread.__init__() not called"
        return self._ident

    if _HAVE_THREAD_NATIVE_ID:
        @property
        def native_id(self):
            assert self._initialized, "Thread.__init__() not called"
            return self._native_id

    @property
    def daemon(self):
        assert self._initialized, "Thread.__init__() not called"
        return self._daemonic

    @daemon.setter
    def daemon(self, daemon):
        if not self._initialized:
            raise RuntimeError("Thread.__init__() not called")
        bool(daemon)
        if self._ident is not None:
            raise RuntimeError("cannot set daemon status of active thread")
        self._daemonic = daemon


class _MainThread(Thread):
    def __init__(self):
        Thread.__init__(self, name="MainThread", daemon=False)
        self._ident = get_ident()
        if _HAVE_THREAD_NATIVE_ID:
            self._native_id = get_native_id()

    def is_alive(self):
        return True


_main_thread = _MainThread()


def main_thread():
    return _main_thread


def current_thread():
    return _imre_thread.current(_main_thread)


def enumerate():
    return [_main_thread] + _imre_thread.threads()


def active_count():
    return len(enumerate())


# CPython 3.14.5, Lib/threading.py. See stdlib/docstrings/LICENSE.
class _RLock:
    """This class implements reentrant lock objects.

    A reentrant lock must be released by the thread that acquired it. Once a
    thread has acquired a reentrant lock, the same thread may acquire it
    again without blocking; the thread must release it once for each time it
    has acquired it.

    """

    def __init__(self):
        self._block = _allocate_lock()
        self._owner = None
        self._count = 0

    def __repr__(self):
        owner = self._owner
        if owner is not None:
            for thread in [_main_thread] + _imre_thread.threads():
                if thread.ident == owner:
                    owner = thread.name
                    break
        return "<%s %s.%s object owner=%r count=%d at %s>" % (
            "locked" if self.locked() else "unlocked",
            self.__class__.__module__,
            self.__class__.__qualname__,
            owner,
            self._count,
            f"0x{id(self):x}"
        )

    def _at_fork_reinit(self):
        self._block._at_fork_reinit()
        self._owner = None
        self._count = 0

    def acquire(self, blocking=True, timeout=-1):
        """Acquire a lock, blocking or non-blocking.

        When invoked without arguments: if this thread already owns the lock,
        increment the recursion level by one, and return immediately. Otherwise,
        if another thread owns the lock, block until the lock is unlocked. Once
        the lock is unlocked (not owned by any thread), then grab ownership, set
        the recursion level to one, and return. If more than one thread is
        blocked waiting until the lock is unlocked, only one at a time will be
        able to grab ownership of the lock. There is no return value in this
        case.

        When invoked with the blocking argument set to true, do the same thing
        as when called without arguments, and return true.

        When invoked with the blocking argument set to false, do not block. If a
        call without an argument would block, return false immediately;
        otherwise, do the same thing as when called without arguments, and
        return true.

        When invoked with the floating-point timeout argument set to a positive
        value, block for at most the number of seconds specified by timeout
        and as long as the lock cannot be acquired.  Return true if the lock has
        been acquired, false if the timeout has elapsed.

        """
        me = get_ident()
        if self._owner == me:
            self._count += 1
            return 1
        rc = self._block.acquire(blocking, timeout)
        if rc:
            self._owner = me
            self._count = 1
        return rc

    __enter__ = acquire

    def release(self):
        """Release a lock, decrementing the recursion level.

        If after the decrement it is zero, reset the lock to unlocked (not owned
        by any thread), and if any other threads are blocked waiting for the
        lock to become unlocked, allow exactly one of them to proceed. If after
        the decrement the recursion level is still nonzero, the lock remains
        locked and owned by the calling thread.

        Only call this method when the calling thread owns the lock. A
        RuntimeError is raised if this method is called when the lock is
        unlocked.

        There is no return value.

        """
        if self._owner != get_ident():
            raise RuntimeError("cannot release un-acquired lock")
        self._count = count = self._count - 1
        if not count:
            self._owner = None
            self._block.release()

    def __exit__(self, t, v, tb):
        self.release()

    def locked(self):
        """Return whether this object is locked."""
        return self._block.locked()

    # Internal methods used by condition variables

    def _acquire_restore(self, state):
        self._block.acquire()
        self._count, self._owner = state

    def _release_save(self):
        if self._count == 0:
            raise RuntimeError("cannot release un-acquired lock")
        count = self._count
        self._count = 0
        owner = self._owner
        self._owner = None
        self._block.release()
        return (count, owner)

    def _is_owned(self):
        return self._owner == get_ident()

    # Internal method used for reentrancy checks

    def _recursion_count(self):
        if self._owner != get_ident():
            return 0
        return self._count

_PyRLock = _RLock
