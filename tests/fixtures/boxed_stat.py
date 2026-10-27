import stat
import sys
from importlib import import_module


class Index:
    def __index__(self):
        return stat.S_IFDIR | stat.S_IRWXU


class BadIndex:
    def __index__(self):
        return "7"


class Store:
    directory = stat.S_ISDIR


def current():
    assert Store.directory is Store().directory is stat.S_ISDIR
    assert Store().directory(stat.S_IFDIR)
    assert import_module("st" + "at") is stat
    mode = stat.S_IFDIR | stat.S_IRWXU
    assert stat.S_ISDIR(mode) and not stat.S_ISDIR(stat.S_IFREG | stat.S_IRWXU)
    assert stat.S_IMODE(mode) == stat.S_IRWXU and stat.filemode(mode) == "drwx------"
    limit = 2**16 if sys.platform == "darwin" else 2**32
    for fn in (stat.S_IMODE, stat.S_ISDIR, stat.filemode):
        print(fn.__module__, fn.__name__, fn.__qualname__)
        for value in (True, Index(), BadIndex(), 1.5, -1, limit - 1, limit, 2**64 - 1, 2**64):
            try:
                print(fn(value))
            except (TypeError, OverflowError) as exc:
                print(type(exc).__name__, str(exc))
        for args, kwargs in (((), {}), ((1, 2), {}), ((), {"mode": 3})):
            try:
                fn(*args, **kwargs)
            except TypeError as exc:
                print(str(exc))
            else:
                raise AssertionError("invalid stat arguments succeeded")
