import os
import posix
from os import RTLD_NODELETE


def current():
    names = sorted(name for name in vars(posix) if name.startswith("RTLD_"))
    assert names == sorted(name for name in vars(os) if name.startswith("RTLD_"))
    assert RTLD_NODELETE is posix.RTLD_NODELETE
    for name in names:
        value = getattr(posix, name)
        assert type(value) is int
        assert vars(posix)[name] is vars(os)[name] is getattr(os, name) is value
        print(name, value)
    assert not hasattr(posix, "RTLD_DEFAULT") and not hasattr(os, "RTLD_NEXT")
