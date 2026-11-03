import os
import sys


def current():
    if sys.platform != "linux":
        return
    import posix

    namespace = vars(os)
    for name in ("fpathconf", "pathconf", "pathconf_names"):
        value = namespace[name]
        assert value is getattr(posix, name)
        delattr(os, name)
        try:
            assert name not in vars(os)
            assert not hasattr(os, name)
            assert getattr(posix, name) is value
        finally:
            setattr(os, name, value)
        assert getattr(os, name) is value
