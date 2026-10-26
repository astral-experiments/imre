from contextlib import AbstractContextManager
import sys


class ExitStack(AbstractContextManager):
    """Context manager for dynamic management of a stack of exit callbacks."""

    _exit_callbacks: list[tuple[bool, object]]

    def __init__(self) -> None:
        self._exit_callbacks = []

    def __enter__(self) -> "ExitStack":
        return self

    def pop_all(self) -> "ExitStack":
        new_stack = type(self)()
        new_stack._exit_callbacks = self._exit_callbacks
        self._exit_callbacks = []
        return new_stack

    def push(self, exit):
        try:
            method = type(exit).__exit__
        except AttributeError:
            self._exit_callbacks.append((True, exit))
        else:
            self._exit_callbacks.append((True, method.__get__(exit)))
        return exit

    def enter_context(self, cm):
        cls = type(cm)
        try:
            enter = cls.__enter__
            exit = cls.__exit__
        except AttributeError:
            raise TypeError(f"'{cls.__module__}.{cls.__qualname__}' object does not support the context manager protocol") from None
        result = enter(cm)
        self._exit_callbacks.append((True, exit.__get__(cm)))
        return result

    def callback(self, callback, /, *args: object, **kwds: object):
        def _exit_wrapper(exc_type: object, exc_value: object, tb: object) -> None:
            callback(*args, **kwds)
        _exit_wrapper.__wrapped__ = callback
        self._exit_callbacks.append((True, _exit_wrapper))
        return callback

    def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
        received = exc is not None
        frame_exc = sys.exception()
        suppressed = False
        pending = False
        while self._exit_callbacks:
            is_sync, cb = self._exit_callbacks.pop()
            assert is_sync
            try:
                if exc is None:
                    exc_details = None, None, None
                else:
                    exc_details = type(exc), exc, exc.__traceback__
                if cb(*exc_details):
                    suppressed = True
                    pending = False
                    exc = None
            except BaseException as new_exc:
                last = new_exc
                while last.__context__ is not None and last.__context__ is not exc:
                    if last.__context__ is frame_exc:
                        last.__context__ = exc
                        break
                    last = last.__context__
                pending = True
                exc = new_exc
        if pending:
            try:
                fixed_context = exc.__context__
                raise exc
            except BaseException:
                exc.__context__ = fixed_context
                raise
        return received and suppressed

    def close(self) -> None:
        self.__exit__(None, None, None)
