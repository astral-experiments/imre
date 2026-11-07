import importlib
import sys
from importlib import util
from importlib.machinery import SourceFileLoader
from types import ModuleType

import price_events


class Prices(ModuleType):
    pass


class PriceLoader(SourceFileLoader):
    def create_module(self, spec):
        return Prices(spec.name)


spec = util.find_spec("price_plugin")
loader = util.LazyLoader(PriceLoader(spec.name, spec.origin))
spec.loader = loader
module = util.module_from_spec(spec)
sys.modules[spec.name] = module
loader.exec_module(module)

assert price_events.calls == []
module.currency = "EUR"
assert module.quote(40) == 42 and module.currency == "EUR"
assert type(module) is Prices and "quote" in Prices.__dir__(module)
assert module.quote.__code__.co_filename == module.__file__
assert importlib.import_module(spec.name) is module
assert module.__loader__ is spec.loader
assert module.quote(41) == 43 and price_events.calls == ["price_plugin"]
print("lazy price plugin")
