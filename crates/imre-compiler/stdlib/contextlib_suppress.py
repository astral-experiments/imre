# CPython 3.14.5 Lib/contextlib.py
import builtins as _builtins

# Exception groups are available only on runtimes that implement PEP 654.
_BaseExceptionGroup = getattr(_builtins, "BaseExceptionGroup", ())


class suppress(AbstractContextManager):
    """Context manager to suppress specified exceptions

    After the exception is suppressed, execution proceeds with the next
    statement following the with statement.

         with suppress(FileNotFoundError):
             os.remove(somefile)
         # Execution still resumes here if the file was already removed
    """

    def __init__(self, *exceptions):
        self._exceptions = exceptions

    def __enter__(self):
        pass

    def __exit__(self, exctype, excinst, exctb):
        if exctype is None:
            return
        if issubclass(exctype, self._exceptions):
            return True
        if issubclass(exctype, _BaseExceptionGroup):
            _, rest = excinst.split(self._exceptions)
            if rest is None:
                return True
            raise rest
        return False
