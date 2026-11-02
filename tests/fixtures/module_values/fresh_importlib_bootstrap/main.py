import importlib
import os
import sys
import tempfile
import warnings
from contextlib import contextmanager

import _frozen_importlib
import _frozen_importlib_external


@contextmanager
def fresh_imports(*blocked):
    saved = {}
    for name in list(sys.modules):
        if name == "importlib" or name.startswith("importlib.") or name in blocked:
            saved[name] = sys.modules.pop(name)
    for name in blocked:
        sys.modules[name] = None
    try:
        yield
    finally:
        for name in list(sys.modules):
            if name == "importlib" or name.startswith("importlib.") or name in blocked:
                del sys.modules[name]
        sys.modules.update(saved)


with fresh_imports():
    frozen = importlib.import_module("importlib")
    frozen_machinery = frozen.import_module("importlib.machinery")

with fresh_imports("_frozen_importlib", "_frozen_importlib_external"):
    source = importlib.import_module("importlib")
    machinery = source.import_module("importlib.machinery")
    core = source._bootstrap
    external = source._bootstrap_external
    assert core is sys.modules["importlib._bootstrap"] and core is not frozen._bootstrap
    assert external is sys.modules["importlib._bootstrap_external"] and external is not frozen._bootstrap_external
    assert external._bootstrap is core and core._bootstrap_external is external
    assert source.__import__ is core.__import__ and source.__import__("sys") is sys
    assert machinery.ModuleSpec is core.ModuleSpec and machinery.ModuleSpec is not frozen_machinery.ModuleSpec
    assert machinery.SourceFileLoader is external.SourceFileLoader
    assert machinery.SourceFileLoader is not frozen_machinery.SourceFileLoader
    with warnings.catch_warnings():
        warnings.filterwarnings("error", category=DeprecationWarning,
                                module=r"^importlib[.]_bootstrap_external$", lineno=258)
        try:
            external.cache_from_source("example.py", False)
        except DeprecationWarning:
            pass
        else:
            raise AssertionError("source bootstrap warning missed its filter")
    original = core._gcd_import
    try:
        def record(name, package, level):
            return name, package, level

        core._gcd_import = record
        assert source.import_module("..child", "parent.pkg") == ("child", "parent.pkg", 2)
    finally:
        core._gcd_import = original
    with tempfile.TemporaryDirectory() as root:
        path = os.path.join(root, "fresh_provider.py")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("answer = 42\n")
        name = "imre_fresh_bootstrap_target"
        loader = machinery.SourceFileLoader(name, path)
        spec = machinery.ModuleSpec(name, loader)
        module = type(sys)(name)
        module.__spec__ = spec
        loader.exec_module(module)
        assert module.answer == 42

assert frozen._bootstrap is _frozen_importlib
assert frozen._bootstrap_external is _frozen_importlib_external
assert sys.modules["importlib"] is importlib
assert sys.modules["_frozen_importlib"] is _frozen_importlib
assert sys.modules["_frozen_importlib_external"] is _frozen_importlib_external

with fresh_imports("_frozen_importlib", "importlib._bootstrap"):
    try:
        importlib.import_module("importlib")
    except ModuleNotFoundError as exc:
        assert exc.name == "importlib._bootstrap"
    else:
        raise AssertionError("disabled core bootstrap was imported")

print("fresh bootstrap providers ok")
