import sys


class Missing(AttributeError):
    pass


calls = []
failure = RuntimeError("lookup failed")
present = "stored"


def __getattr__(name):
    calls.append(name)
    if name == "fresh":
        return []
    if name == "cached":
        globals()[name] = []
        return globals()[name]
    if name == "broken":
        raise failure
    raise Missing(name)


def current():
    module = sys.modules[__name__]
    assert module.present == "stored"
    assert module.fresh is not module.fresh
    assert module.cached is module.cached
    assert calls == ["fresh", "fresh", "cached"]
    assert module.__annotate__ is None
    assert module.__annotations__ is module.__annotations__
    module.__annotate__ = lambda fmt: {"format": fmt}
    assert module.__annotations__ == {"format": 1}
    assert module.__init__.__self__ is module
    assert module.__repr__() == repr(module) == module.__str__()
    assert "present" in module.__dir__()
    assert calls == ["fresh", "fresh", "cached"]
    try:
        object.__getattribute__(module, "fresh")
    except AttributeError:
        pass
    else:
        raise AssertionError("object lookup called the module hook")
    assert module.__getattribute__("fresh") == []
    assert not hasattr(module, "absent")
    assert getattr(module, "absent", 10) == 10
    try:
        getattr(module, "broken", None)
    except RuntimeError as error:
        assert error is failure
    else:
        raise AssertionError("module lookup suppressed RuntimeError")

    class Name(str):
        pass

    name = Name("fresh")
    getattr(module, name)
    assert calls[-1] is name
    # A synthesized native export is visible before a module namespace hook.
    import math
    math.__getattr__ = lambda key: name
    try:
        assert math.floor(2.5) == 2
        assert math.absent is name
    finally:
        del math.__getattr__
    # The importer's path probe precedes all member lookups, including hits.
    original = module.__getattr__

    def path_error(key):
        if key == "__path__":
            raise failure
        raise Missing(key)

    module.__getattr__ = path_error
    try:
        try:
            __import__(__name__, fromlist=("present",))
        except RuntimeError as exc:
            assert exc is failure
        else:
            raise AssertionError("fromlist suppressed path lookup failure")
        try:
            __import__(__name__ + ".child")
        except RuntimeError as exc:
            assert exc is failure
        else:
            raise AssertionError("child import suppressed path lookup failure")
    finally:
        module.__getattr__ = original
    # A populated __all__ never needs the package name to fetch a child.
    from types import ModuleType
    package = ModuleType("hook_pkg")
    package.__path__ = []
    package.__all__ = ["present"]
    package.present = present
    del package.__name__
    package.__getattr__ = lambda key: calls.append(key)
    sys.modules["hook_pkg"] = package
    try:
        before = list(calls)
        assert __import__("hook_pkg", fromlist=("*",)) is package
        assert calls == before
    finally:
        del sys.modules["hook_pkg"]
    print("source module getattr")
