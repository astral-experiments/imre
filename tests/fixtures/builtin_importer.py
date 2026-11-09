import importlib
import sys
from importlib import util
from importlib.machinery import BuiltinImporter

name = "errno"
spec = BuiltinImporter.find_spec(name)
assert spec.loader is BuiltinImporter and spec.origin == "built-in"
assert not spec.has_location and spec.submodule_search_locations is None
assert BuiltinImporter.get_code(name) is BuiltinImporter.get_source(name) is None

first = importlib.import_module(name)
assert util.find_spec(name) is first.__spec__
assert first.__loader__ is BuiltinImporter
del sys.modules[name]
try:
    assert util.find_spec(name).loader is BuiltinImporter
    assert name not in sys.modules
    fresh = importlib.import_module(name)
    assert fresh is sys.modules[name] and fresh is not first
    assert fresh.EBADF == first.EBADF
    fresh.EBADF = -1
    assert BuiltinImporter.exec_module(fresh) is None
    assert fresh.EBADF == -1
finally:
    sys.modules[name] = first

assert BuiltinImporter.find_spec("price_plugin") is None
assert BuiltinImporter.find_spec("missing_price_plugin") is None
assert util.find_spec("price_plugin").has_location
assert util.find_spec("missing_price_plugin") is None
plugin = importlib.import_module("price_plugin")
assert plugin.quote(3) == 21
assert BuiltinImporter.exec_module(plugin) is None
assert plugin.executions == 1
names = sys.builtin_module_names
assert names is sys.__dict__["builtin_module_names"] and type(names) is tuple
assert names == tuple(sorted(set(names)))
assert name in names and "price_plugin" not in names
for item in names:
    assert BuiltinImporter.find_spec(item).origin == "built-in"

sys.builtin_module_names = ("price_plugin",)
try:
    assert BuiltinImporter.find_spec(name).loader is BuiltinImporter
    assert BuiltinImporter.find_spec("price_plugin") is None
    assert BuiltinImporter.get_code("price_plugin") is None
    assert BuiltinImporter.create_module(type(spec)("price_plugin", BuiltinImporter)) is None
    del sys.builtin_module_names
    assert not hasattr(sys, "builtin_module_names")
finally:
    sys.builtin_module_names = names

print("builtin and source imports")
