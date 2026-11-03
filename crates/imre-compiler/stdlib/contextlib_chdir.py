# CPython 3.14.5 Lib/contextlib.py
import os as _os


class chdir(AbstractContextManager):
    """Non thread-safe context manager to change the current working directory."""

    def __init__(self, path):
        self.path = path
        self._old_cwd = []

    def __enter__(self):
        self._old_cwd.append(_os.getcwd())
        _os.chdir(self.path)

    def __exit__(self, *excinfo):
        _os.chdir(self._old_cwd.pop())
