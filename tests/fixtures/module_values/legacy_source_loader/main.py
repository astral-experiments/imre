import os
import sys
import tempfile
from importlib.machinery import SourceFileLoader
from types import ModuleType
import warnings


class RecordingLoader(SourceFileLoader):
    created = 0

    def create_module(self, spec):
        self.created += 1
        return ModuleType(spec.name)


class ReplacingLoader(SourceFileLoader):
    def exec_module(self, module):
        sys.modules[self.name] = self
        if self.fail:
            raise LookupError("initializer failed")


message = "the load_module() method is deprecated and slated for removal in Python 3.15; use exec_module() instead"
with tempfile.TemporaryDirectory() as root:
    path = os.path.join(root, "plugin.py")
    with open(path, "w", encoding="utf-8") as f:
        f.write("import sys\nowner = sys.modules[__name__]\ninitializing = getattr(__spec__, '_initializing', None)\n")
    name = "direct_loader_plugin"
    loader = RecordingLoader(name, path)
    with warnings.catch_warnings(record=True) as seen:
        warnings.simplefilter("always", DeprecationWarning)
        try:
            loader.load_module("other")
        except ImportError as exc:
            assert exc.name == "other" and not seen
        else:
            raise AssertionError("mismatched name accepted")
        fresh = loader.load_module()
        assert fresh is sys.modules[name] and fresh.owner is fresh and fresh.initializing is True
        assert fresh.__loader__ is loader and fresh.__spec__.loader is loader
        assert fresh.__file__ == path and fresh.__package__ == ""
        assert not fresh.__spec__._initializing and loader.created == 1
        assert len(seen) == 1 and seen[0].category is DeprecationWarning
        assert str(seen[0].message) == message
        assert seen[0].filename == "<frozen importlib._bootstrap>" and seen[0].lineno == 533

        fresh.__name__ = "old name"
        fresh.__file__ = "old file"
        again = loader.load_module(name=name)
        assert again is fresh and again.owner is fresh and loader.created == 1
        assert again.initializing is None and again.__file__ == path and again.__name__ == name

        replacement = ReplacingLoader(name, path)
        replacement.fail = False
        assert replacement.load_module(None) is replacement and sys.modules.pop(name) is replacement
        replacement.fail = True
        try:
            replacement.load_module()
        except LookupError as exc:
            assert str(exc) == "initializer failed"
        else:
            raise AssertionError("failed fresh load succeeded")
        assert name not in sys.modules
        sys.modules[name] = fresh
        try:
            replacement.load_module()
        except LookupError:
            pass
        else:
            raise AssertionError("failed reload succeeded")
        assert sys.modules.pop(name) is replacement

        with open(path, "w", encoding="utf-8") as f:
            f.write("=")
        try:
            loader.load_module()
        except SyntaxError:
            pass
        else:
            raise AssertionError("invalid source accepted")
        assert name not in sys.modules

    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        warnings.filterwarnings("error", category=DeprecationWarning,
                                module=r"^importlib[.]_bootstrap$", lineno=533)
        try:
            loader.load_module()
        except DeprecationWarning as exc:
            assert str(exc) == message
        else:
            raise AssertionError("frozen bootstrap warning ignored")
    assert name not in sys.modules

print("legacy loader ok")
