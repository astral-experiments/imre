import _imp
import sys
from importlib import import_module, util
from importlib.machinery import BuiltinImporter, FrozenImporter

name = "__hello__"
assert sys.meta_path.index(BuiltinImporter) < sys.meta_path.index(FrozenImporter)
assert FrozenImporter.find_spec("sys") is None
assert FrozenImporter.find_spec("frozen_price_plugin") is None

_imp._override_frozen_modules_for_tests(-1)
try:
    assert FrozenImporter.find_spec(name) is None
    assert FrozenImporter.find_spec("_frozen_importlib").loader is FrozenImporter
finally:
    _imp._override_frozen_modules_for_tests(0)

sys.modules.pop(name, None)
spec = util.find_spec(name)
assert spec.origin == "frozen" and spec.loader is FrozenImporter
assert FrozenImporter.get_source(name) is None
code = spec.loader.get_code(name)
assert code.co_filename == "<frozen __hello__>"
module = util.module_from_spec(spec)
assert name not in sys.modules and module.__spec__ is spec
spec.loader.exec_module(module)
assert module.initialized and name not in sys.modules
old = module.TestFrozenUtf8_1
module.initialized = False
spec.loader.exec_module(module)
assert module.initialized and module.TestFrozenUtf8_1 is not old

imported = import_module(name)
assert imported is sys.modules[name] and imported is not module
assert util.find_spec(name) is imported.__spec__
assert imported.__loader__ is FrozenImporter
assert util.find_spec("frozen_price_plugin").has_location
assert import_module("frozen_price_plugin").quote(3) == 21
print("frozen and source imports")
