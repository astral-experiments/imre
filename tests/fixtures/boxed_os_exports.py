from os import *

import os
import posix
import sys
from importlib import import_module
from types import ModuleType


def current():
    exports = os.__all__
    assert isinstance(exports, list) and len(exports) == len(set(exports))
    assert set(name for name in dir(posix) if not name.startswith("_")) <= set(exports)
    assert all(globals()[name] is getattr(os, name) for name in exports)
    assert "open" in exports and "walk" in exports and "supports_fd" not in exports
    assert "sys" not in exports and "GenericAlias" not in exports
    assert ("_exit" in exports) == hasattr(posix, "_exit")
    assert environ is os.environ and environ is not posix.environ
    for module, peer in ((os, posix), (posix, os)):
        fresh = ModuleType(module.__name__)
        fresh.local = fresh
        assert "local" in dir(fresh) and "open" not in dir(fresh)
        assert not hasattr(fresh, "open")
        original, other = module.open, peer.open
        try:
            del module.open
            assert "open" not in vars(module) and "open" not in dir(module)
            assert not hasattr(module, "open")
            assert peer.open is other
            module.open = fresh
            assert module.open is fresh and "open" in dir(module)
        finally:
            module.open = original
        assert module.open is original
    for name in ("fork", "eventfd"):
        if hasattr(posix, name):
            original = getattr(posix, name)
            try:
                delattr(posix, name)
                del sys.modules["os"]
                fresh_os = import_module("os")
                assert not hasattr(fresh_os, name) and name not in fresh_os.__all__
            finally:
                setattr(posix, name, original)
                sys.modules["os"] = os
    return "ok"
