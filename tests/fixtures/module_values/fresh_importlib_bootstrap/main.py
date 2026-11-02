import _imp
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
    for key in ("SOURCE_SUFFIXES", "BYTECODE_SUFFIXES", "EXTENSION_SUFFIXES"):
        assert getattr(machinery, key) is getattr(external, key)
        assert getattr(frozen_machinery, key) is getattr(_frozen_importlib_external, key)
        assert getattr(machinery, key) is not getattr(frozen_machinery, key)
    ext = _imp.extension_suffixes()
    assert ext == machinery.EXTENSION_SUFFIXES and ext is not machinery.EXTENSION_SUFFIXES
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

    source_only = source.import_module("importlib.abc")
    assert issubclass(machinery.FileFinder, source_only.PathEntryFinder)
    assert not issubclass(frozen_machinery.FileFinder, source_only.PathEntryFinder)

    # Keep the source machinery facade while the two private frozen providers are available.
    sys.modules["_frozen_importlib"] = _frozen_importlib
    sys.modules["_frozen_importlib_external"] = _frozen_importlib_external
    del sys.modules["importlib.abc"]
    protocols = source.import_module("importlib.abc")
    for provider in (machinery, frozen_machinery):
        assert issubclass(provider.FileFinder, protocols.PathEntryFinder)
        assert issubclass(provider.SourceFileLoader, protocols.FileLoader)
        assert issubclass(provider.NamespaceLoader, protocols.InspectLoader)

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

with tempfile.TemporaryDirectory() as root:
    first = os.path.join(root, "first")
    second = os.path.join(root, "second")
    os.mkdir(first)
    os.mkdir(second)
    first_portion = os.path.join(first, "imre_portion")
    second_portion = os.path.join(second, "imre_portion")
    os.mkdir(first_portion)
    sys.path.insert(0, second)
    sys.path.insert(0, first)
    try:
        portion = source.import_module("imre_portion")
        assert type(portion.__path__) is _frozen_importlib_external._NamespacePath
        assert type(portion.__loader__) is external.NamespaceLoader
        assert portion.__loader__ is portion.__spec__.loader
        assert list(portion.__path__) == [first_portion]
        os.mkdir(second_portion)
        assert list(portion.__path__) == [first_portion]
        source.invalidate_caches()
        assert list(portion.__path__) == [first_portion, second_portion]
    finally:
        sys.path.remove(first)
        sys.path.remove(second)
        sys.modules.pop("imre_portion", None)


print("fresh bootstrap providers ok")
