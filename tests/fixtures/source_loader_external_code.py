from importlib.machinery import SourceFileLoader
from types import ModuleType
import local_body

# A new loader for an imported module must compile the current file.
origin = local_body.__file__
loader = SourceFileLoader(local_body.__name__, origin)
with open(origin, "rb") as stream:
    original = stream.read()
try:
    with open(origin, "wb") as stream:
        stream.write(b"value = 'changed on disk'\n")
    namespace = {}
    exec(loader.get_code(local_body.__name__), namespace)
    assert namespace["value"] == "changed on disk"
    module = ModuleType(local_body.__name__)
    loader.exec_module(module)
    assert module.value == "changed on disk"
finally:
    with open(origin, "wb") as stream:
        stream.write(original)
