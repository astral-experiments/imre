import _thread
import io
import sys


def current():
    # A lookup on _thread can import threading again while its hook is being bound.
    original_hook = getattr(_thread, "_excepthook", None)
    if original_hook is not None:
        del _thread._excepthook
    observed = []

    def low_attr(name):
        if name == "_excepthook":
            partial = sys.modules["threading"]
            assert hasattr(partial, "Thread") and not hasattr(partial, "_main_thread")
            assert not any(hasattr(partial, key) for key in ("current_thread", "main_thread", "active_count", "enumerate"))
            lock = partial.RLock()
            with partial.Condition(lock) as acquired:
                assert acquired
            assert not partial.Event().is_set()
            observed.append(name)
            if original_hook is not None:
                return original_hook
        raise AttributeError(name)

    _thread.__getattr__ = low_attr
    try:
        from threading import ExceptHookArgs, Thread, __excepthook__, excepthook, get_ident
    finally:
        del _thread.__getattr__
        if original_hook is not None:
            _thread._excepthook = original_hook
    assert observed == ["_excepthook"]

    assert __excepthook__ is excepthook

    class CheckedThread(Thread):
        def __setattr__(self, name, value):
            if name == "_stderr":
                assert self._initialized
            super().__setattr__(name, value)

    saved = io.StringIO()
    real = io.StringIO()
    old_stderr = sys.stderr
    old_real = sys.__stderr__
    try:
        sys.stderr = saved
        worker = CheckedThread(name="printer")
        assert worker._stderr is saved
        try:
            raise ValueError("callback failure")
        except ValueError as error:
            args = ExceptHookArgs((ValueError, error, error.__traceback__, worker))
            assert args.thread is worker and args.exc_value is error
            assert args.exc_traceback is error.__traceback__
            excepthook(ExceptHookArgs((SystemExit, None, None, worker)))
            assert saved.getvalue() == ""
            excepthook(ExceptHookArgs((ValueError, error, error.__traceback__, None)))
            assert saved.getvalue().startswith(f"Exception in thread {get_ident()}:\n")
            assert saved.getvalue().endswith("ValueError: callback failure\n")
            saved.seek(0)
            saved.truncate()
            sys.stderr = None
            sys.__stderr__ = real
            excepthook(args)
        assert saved.getvalue().startswith("Exception in thread printer:\n")
        if excepthook is getattr(_thread, "_excepthook", None):
            assert saved.getvalue() == "Exception in thread printer:\n"
            assert real.getvalue().endswith("ValueError: callback failure\n")
        else:
            assert saved.getvalue().endswith("ValueError: callback failure\n")
            assert real.getvalue() == ""
    finally:
        sys.stderr = old_stderr
        sys.__stderr__ = old_real
    print("thread hook ok")


if __name__ == "__main__":
    current()
