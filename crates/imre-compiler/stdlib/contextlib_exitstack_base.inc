    _exit_callbacks: list[tuple[bool, object]]

    def __init__(self) -> None:
        self._exit_callbacks = []

    def pop_all(self):
        """Preserve the context stack by transferring it to a new instance."""
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
            self._exit_callbacks.append((True, types.MethodType(method, exit)))
        return exit

    def enter_context(self, cm):
        cls = type(cm)
        try:
            enter = cls.__enter__
            exit = cls.__exit__
        except AttributeError:
            raise TypeError(f"'{cls.__module__}.{cls.__qualname__}' object does not support the context manager protocol") from None
        result = enter(cm)
        self._exit_callbacks.append((True, types.MethodType(exit, cm)))
        return result

    def callback(self, callback, /, *args: object, **kwds: object):
        def _exit_wrapper(exc_type, exc, tb):
            callback(*args, **kwds)
        _exit_wrapper.__wrapped__ = callback
        self._exit_callbacks.append((True, _exit_wrapper))
        return callback
