import os
import sys
from importlib.machinery import SourceFileLoader
from types import ModuleType
import local_body

# The stock loader reloads source. A native loader can reuse its registered body.
linked = len(sys.argv) == 2
expected = int(sys.argv[1]) if linked else "changed on disk"
origin = local_body.__file__
loader = SourceFileLoader(local_body.__name__, origin)
with open(origin, "rb") as stream:
    original = stream.read()
try:
    with open(origin, "wb") as stream:
        stream.write(b"value = 'changed on disk'\n")
    namespace = {}
    exec(loader.get_code(local_body.__name__), namespace)
    assert namespace["value"] == expected
    module = ModuleType(local_body.__name__)
    loader.exec_module(module)
    assert module.value == expected

    with open(origin, "wb") as stream:
        stream.write(b"def broken(:\n")
    if linked:
        loader.exec_module(module)
    else:
        try:
            loader.exec_module(module)
        except SyntaxError as error:
            assert error.filename == origin and error.lineno == 1
        else:
            raise AssertionError("executed invalid physical source")
    assert module.value == expected

    os.unlink(origin)
    if linked:
        namespace = {}
        exec(loader.get_code(local_body.__name__), namespace)
        assert namespace["value"] == expected
    else:
        try:
            loader.get_code(local_body.__name__)
        except FileNotFoundError as error:
            assert error.filename == origin
        else:
            raise AssertionError("loaded code after the physical source was deleted")
finally:
    with open(origin, "wb") as stream:
        stream.write(original)
