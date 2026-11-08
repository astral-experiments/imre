import os
import sys
from functools import partial
from importlib.machinery import SourceFileLoader
from types import FunctionType, ModuleType
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

# A replacement compiler owns its operands, even when they have no filesystem spelling.
class Operand:
    def __fspath__(self):
        raise AssertionError("converted the custom compiler's path")

    def __index__(self):
        raise AssertionError("converted the custom compiler's optimize argument")


calls = []


def custom(source, filename, mode, *, dont_inherit, optimize):
    calls.append((source, filename, mode, dont_inherit, optimize))
    return result


method = loader.source_to_code.__func__
convert = FunctionType(method.__code__, dict(method.__globals__, compile=custom))
payload, filename, optimize, result = bytearray(b"value = 7"), Operand(), Operand(), object()
assert convert(loader, payload, filename, _optimize=optimize) is result
assert calls == [(payload, filename, "exec", True, optimize)]

# Forwarding the untouched objects through a partial still lets the builtin convert them.
class DisplayPath:
    def __fspath__(self):
        calls.append("path")
        return b"display.py"


class Optimize:
    def __index__(self):
        calls.append("index")
        return 0


forward = FunctionType(method.__code__, dict(method.__globals__, compile=partial(compile)))
calls.clear()
code = forward(loader, payload, DisplayPath(), _optimize=Optimize())
assert calls == ["path", "index"]
assert code.co_filename == "display.py"


# Changing the shipped method's keyword default can run Python during a registered load.
class Uncached(SourceFileLoader):
    def path_stats(self, path):
        raise OSError("read the source")


defaults = method.__kwdefaults__
try:
    method.__kwdefaults__ = {"_optimize": Optimize()}
    calls.clear()
    Uncached(local_body.__name__, origin).get_code(local_body.__name__)
    assert calls == ["index"]
finally:
    method.__kwdefaults__ = defaults
