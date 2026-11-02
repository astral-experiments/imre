"""Thread objects for the native object runtime."""

from _thread import TIMEOUT_MAX, LockType as Lock, _local as local, get_ident
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
