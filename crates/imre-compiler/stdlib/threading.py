"""Thread objects for the native object runtime."""

from _thread import TIMEOUT_MAX, LockType as Lock, _local as local, get_ident
from _thread import allocate_lock as _allocate_lock
import _thread
from time import monotonic as _time
from _weakrefset import WeakSet
from itertools import count
import _imre_thread
try:
    from _collections import deque as _deque
except ImportError:
    from collections import deque as _deque


__all__ = ["Thread", "Lock", "RLock", "Condition", "Event", "TIMEOUT_MAX", "local", "get_ident",
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

    def isDaemon(self):
        """Return whether this thread is a daemon.

        This method is deprecated, use the daemon attribute instead.

        """
        import warnings
        warnings.warn('isDaemon() is deprecated, get the daemon attribute instead',
                      DeprecationWarning, stacklevel=2)
        return self.daemon

    def setDaemon(self, daemonic):
        """Set whether this thread is a daemon.

        This method is deprecated, use the .daemon property instead.

        """
        import warnings
        warnings.warn('setDaemon() is deprecated, set the daemon attribute instead',
                      DeprecationWarning, stacklevel=2)
        self.daemon = daemonic

    def getName(self):
        """Return a string used for identification purposes only.

        This method is deprecated, use the name attribute instead.

        """
        import warnings
        warnings.warn('getName() is deprecated, get the name attribute instead',
                      DeprecationWarning, stacklevel=2)
        return self.name

    def setName(self, name):
        """Set the name string for this thread.

        This method is deprecated, use the name attribute instead.

        """
        import warnings
        warnings.warn('setName() is deprecated, set the name attribute instead',
                      DeprecationWarning, stacklevel=2)
        self.name = name


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


def currentThread():
    """Return the current Thread object, corresponding to the caller's thread of control.

    This function is deprecated, use current_thread() instead.

    """
    import warnings
    warnings.warn('currentThread() is deprecated, use current_thread() instead',
                  DeprecationWarning, stacklevel=2)
    return current_thread()

def activeCount():
    """Return the number of Thread objects currently alive.

    This function is deprecated, use active_count() instead.

    """
    import warnings
    warnings.warn('activeCount() is deprecated, use active_count() instead',
                  DeprecationWarning, stacklevel=2)
    return active_count()


# CPython 3.14.5, Lib/threading.py. See stdlib/docstrings/LICENSE.
def RLock(*args, **kwargs):
    """Factory function that returns a new reentrant lock.

    A reentrant lock must be released by the thread that acquired it. Once a
    thread has acquired a reentrant lock, the same thread may acquire it again
    without blocking; the thread must release it once for each time it has
    acquired it.

    """
    if args or kwargs:
        import warnings
        warnings.warn(
            'Passing arguments to RLock is deprecated and will be removed in 3.15',
            DeprecationWarning,
            stacklevel=2,
        )
    if _CRLock is None:
        return _PyRLock(*args, **kwargs)
    return _CRLock(*args, **kwargs)

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


class Condition:
    """Class that implements a condition variable.

    A condition variable allows one or more threads to wait until they are
    notified by another thread.

    If the lock argument is given and not None, it must be a Lock or RLock
    object, and it is used as the underlying lock. Otherwise, a new RLock object
    is created and used as the underlying lock.

    """

    def __init__(self, lock=None):
        if lock is None:
            lock = RLock()
        self._lock = lock
        # Export the lock's acquire(), release(), and locked() methods
        self.acquire = lock.acquire
        self.release = lock.release
        self.locked = lock.locked
        # If the lock defines _release_save() and/or _acquire_restore(),
        # these override the default implementations (which just call
        # release() and acquire() on the lock).  Ditto for _is_owned().
        if hasattr(lock, '_release_save'):
            self._release_save = lock._release_save
        if hasattr(lock, '_acquire_restore'):
            self._acquire_restore = lock._acquire_restore
        if hasattr(lock, '_is_owned'):
            self._is_owned = lock._is_owned
        self._waiters = _deque()

    def _at_fork_reinit(self):
        self._lock._at_fork_reinit()
        self._waiters.clear()

    def __enter__(self):
        return self._lock.__enter__()

    def __exit__(self, *args):
        return self._lock.__exit__(*args)

    def __repr__(self):
        return "<Condition(%s, %d)>" % (self._lock, len(self._waiters))

    def _release_save(self):
        self._lock.release()           # No state to save

    def _acquire_restore(self, x):
        self._lock.acquire()           # Ignore saved state

    def _is_owned(self):
        # Return True if lock is owned by current_thread.
        # This method is called only if _lock doesn't have _is_owned().
        if self._lock.acquire(False):
            self._lock.release()
            return False
        else:
            return True

    def wait(self, timeout=None):
        """Wait until notified or until a timeout occurs.

        If the calling thread has not acquired the lock when this method is
        called, a RuntimeError is raised.

        This method releases the underlying lock, and then blocks until it is
        awakened by a notify() or notify_all() call for the same condition
        variable in another thread, or until the optional timeout occurs. Once
        awakened or timed out, it re-acquires the lock and returns.

        When the timeout argument is present and not None, it should be a
        floating-point number specifying a timeout for the operation in seconds
        (or fractions thereof).

        When the underlying lock is an RLock, it is not released using its
        release() method, since this may not actually unlock the lock when it
        was acquired multiple times recursively. Instead, an internal interface
        of the RLock class is used, which really unlocks it even when it has
        been recursively acquired several times. Another internal interface is
        then used to restore the recursion level when the lock is reacquired.

        """
        if not self._is_owned():
            raise RuntimeError("cannot wait on un-acquired lock")
        waiter = _allocate_lock()
        waiter.acquire()
        self._waiters.append(waiter)
        saved_state = self._release_save()
        gotit = False
        try:    # restore state no matter what (e.g., KeyboardInterrupt)
            if timeout is None:
                waiter.acquire()
                gotit = True
            else:
                if timeout > 0:
                    gotit = waiter.acquire(True, timeout)
                else:
                    gotit = waiter.acquire(False)
            return gotit
        finally:
            self._acquire_restore(saved_state)
            if not gotit:
                try:
                    self._waiters.remove(waiter)
                except ValueError:
                    pass

    def wait_for(self, predicate, timeout=None):
        """Wait until a condition evaluates to True.

        predicate should be a callable which result will be interpreted as a
        boolean value.  A timeout may be provided giving the maximum time to
        wait.

        """
        endtime = None
        waittime = timeout
        result = predicate()
        while not result:
            if waittime is not None:
                if endtime is None:
                    endtime = _time() + waittime
                else:
                    waittime = endtime - _time()
                    if waittime <= 0:
                        break
            self.wait(waittime)
            result = predicate()
        return result

    def notify(self, n=1):
        """Wake up one or more threads waiting on this condition, if any.

        If the calling thread has not acquired the lock when this method is
        called, a RuntimeError is raised.

        This method wakes up at most n of the threads waiting for the condition
        variable; it is a no-op if no threads are waiting.

        """
        if not self._is_owned():
            raise RuntimeError("cannot notify on un-acquired lock")
        waiters = self._waiters
        while waiters and n > 0:
            waiter = waiters[0]
            try:
                waiter.release()
            except RuntimeError:
                # gh-92530: The previous call of notify() released the lock,
                # but was interrupted before removing it from the queue.
                # It can happen if a signal handler raises an exception,
                # like CTRL+C which raises KeyboardInterrupt.
                pass
            else:
                n -= 1
            try:
                waiters.remove(waiter)
            except ValueError:
                pass

    def notify_all(self):
        """Wake up all threads waiting on this condition.

        If the calling thread has not acquired the lock when this method
        is called, a RuntimeError is raised.

        """
        self.notify(len(self._waiters))

    def notifyAll(self):
        """Wake up all threads waiting on this condition.

        This method is deprecated, use notify_all() instead.

        """
        import warnings
        warnings.warn('notifyAll() is deprecated, use notify_all() instead',
                      DeprecationWarning, stacklevel=2)
        self.notify_all()


class Event:
    """Class implementing event objects.

    Events manage a flag that can be set to true with the set() method and reset
    to false with the clear() method. The wait() method blocks until the flag is
    true.  The flag is initially false.

    """

    # After Tim Peters' event class (without is_posted())

    def __init__(self):
        self._cond = Condition(Lock())
        self._flag = False

    def __repr__(self):
        cls = self.__class__
        status = 'set' if self._flag else 'unset'
        return f"<{cls.__module__}.{cls.__qualname__} at {id(self):#x}: {status}>"

    def _at_fork_reinit(self):
        # Private method called by Thread._after_fork()
        self._cond._at_fork_reinit()

    def is_set(self):
        """Return true if and only if the internal flag is true."""
        return self._flag

    def isSet(self):
        """Return true if and only if the internal flag is true.

        This method is deprecated, use is_set() instead.

        """
        import warnings
        warnings.warn('isSet() is deprecated, use is_set() instead',
                      DeprecationWarning, stacklevel=2)
        return self.is_set()

    def set(self):
        """Set the internal flag to true.

        All threads waiting for it to become true are awakened. Threads
        that call wait() once the flag is true will not block at all.

        """
        with self._cond:
            self._flag = True
            self._cond.notify_all()

    def clear(self):
        """Reset the internal flag to false.

        Subsequently, threads calling wait() will block until set() is called to
        set the internal flag to true again.

        """
        with self._cond:
            self._flag = False

    def wait(self, timeout=None):
        """Block until the internal flag is true.

        If the internal flag is true on entry, return immediately. Otherwise,
        block until another thread calls set() to set the flag to true, or until
        the optional timeout occurs.

        When the timeout argument is present and not None, it should be a
        floating-point number specifying a timeout for the operation in seconds
        (or fractions thereof).

        This method returns the internal flag on exit, so it will always return
        ``True`` except if a timeout is given and the operation times out, when
        it will return ``False``.

        """
        with self._cond:
            signaled = self._flag
            if not signaled:
                signaled = self._cond.wait(timeout)
            return signaled
