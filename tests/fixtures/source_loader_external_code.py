import os
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

    with open(origin, "wb") as stream:
        stream.write(b"def broken(:\n")
    try:
        loader.exec_module(module)
    except SyntaxError as error:
        assert error.filename == origin and error.lineno == 1
    else:
        raise AssertionError("executed invalid physical source")
    assert module.value == "changed on disk"

    os.unlink(origin)
    try:
        loader.get_code(local_body.__name__)
    except FileNotFoundError as error:
        assert error.filename == origin
    else:
        raise AssertionError("loaded code after the physical source was deleted")
finally:
    with open(origin, "wb") as stream:
        stream.write(original)
