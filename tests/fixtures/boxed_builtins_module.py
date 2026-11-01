import builtins
from builtins import len as original_len
from types import ModuleType


def measure():
    return len("abc")


def read_late():
    return late_builtin


assert type(builtins) is ModuleType
assert __name__ == "__main__"
assert globals()["__builtins__"] is __builtins__ is builtins
assert builtins.len is original_len is len
assert builtins.list is list and builtins.KeyError is KeyError

namespace = {}
exec("def measure(): return len('abc')", namespace)
assert namespace["__builtins__"] is builtins.__dict__
override = {"__builtins__": {"len": lambda value: 23}}
exec("def measure(): return len('abc')", override)

try:
    builtins.len = lambda value: 17
    from builtins import len as replacement

    assert builtins.__dict__["len"] is replacement
    assert measure() == namespace["measure"]() == 17
    assert override["measure"]() == 23
    assert original_len("abc") == 3
    del builtins.len
    assert not hasattr(builtins, "len")
    try:
        measure()
    except NameError:
        pass
    else:
        raise AssertionError("deleted builtin still resolves")
finally:
    builtins.len = original_len
assert measure() == namespace["measure"]() == 3

marker = object()
builtins.__dict__["late_builtin"] = marker
try:
    assert builtins.late_builtin is marker and read_late() is marker
    late_builtin = object()
    assert read_late() is late_builtin and late_builtin is not marker
    del late_builtin
    assert read_late() is marker
finally:
    del builtins.late_builtin
assert "late_builtin" not in builtins.__dict__
try:
    builtins.late_builtin
except AttributeError:
    pass
else:
    raise AssertionError("deleted attribute still resolves")
print("builtins namespace ok")
