import builtins
from builtins import len as original_len
from types import ModuleType


def measure():
    return len("abc")


def read_late():
    return late_builtin


assert vars(builtins)["__doc__"] is builtins.__doc__
assert builtins.__doc__.startswith("Built-in functions, types, exceptions, and other objects.")
assert type(builtins) is ModuleType
assert __name__ == "__main__"
assert globals()["__builtins__"] is __builtins__ is builtins
assert builtins.len is original_len is len
assert builtins.list is list and builtins.KeyError is KeyError
assert builtins.SystemError is SystemError
assert SystemError.__mro__ == (SystemError, Exception, BaseException, object)


class RegistryError(LookupError, SystemError):
    pass


try:
    raise RegistryError("codec")
except SystemError as error:
    assert isinstance(error, LookupError) and error.args == ("codec",)
else:
    raise AssertionError("SystemError failed to catch its subclass")

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

class Namespace:
    pass


record = Namespace()
record.a = 1
namespace = vars(record)
assert namespace is record.__dict__ is vars(record)
assert namespace == {"a": 1}
namespace.update({"a": 2, "b": 3})
assert (record.a, record.b) == (2, 3)
del record.b
assert "b" not in namespace
assert vars(builtins) is builtins.__dict__


class Custom:
    @property
    def __dict__(self):
        return marker


assert vars(Custom()) is marker


class Broken:
    @property
    def __dict__(self):
        raise ValueError("descriptor failed")


try:
    vars(Broken())
except ValueError as error:
    assert str(error) == "descriptor failed"
else:
    raise AssertionError("vars swallowed a descriptor exception")


for call, message in [
    (lambda: vars(42), "vars() argument must have __dict__ attribute"),
    (lambda: vars(1, 2), "vars expected at most 1 argument, got 2"),
    (lambda: vars(obj=record), "vars() takes no keyword arguments"),
]:
    try:
        call()
    except TypeError as error:
        assert str(error) == message
    else:
        raise AssertionError(message)

print("builtins namespace ok")
