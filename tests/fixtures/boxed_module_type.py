def current():
    import sys
    import types
    import _weakref
    from types import ModuleType

    assert not hasattr(types, "absent_from_imported_module")
    assert type(sys) is type(types) is ModuleType
    assert (ModuleType.__name__, ModuleType.__module__) == ("module", "builtins")

    class TemplateModule(ModuleType):
        def __init__(self, name):
            super().__init__(name, "template package")

        def label(self):
            return self.__name__

    class ReadHooks(ModuleType):
        def __getattribute__(self, name):
            if name == "answer":
                return 42
            return super().__getattribute__(name)

        def __getattr__(self, name):
            return "fallback"

    hooks = ReadHooks("custom")
    hooks.__dict__["answer"] = 7
    assert hooks.answer == 42
    assert hooks.missing == "fallback"

    fresh = ModuleType(name="sys", doc=17)
    assert fresh is not sys and not hasattr(fresh, "stdout")
    bindings = fresh.__dict__
    fresh.answer = 42
    fresh.__init__("configured")
    assert fresh.__dict__ is bindings and bindings["answer"] == 42
    bindings["answer"] = 84
    assert fresh.answer == 84
    del fresh.answer
    assert "answer" not in bindings

    fresh.__annotate__ = lambda form: {"format": form}
    try:
        del fresh.__annotations__
    except AttributeError as exc:
        assert str(exc) == "__annotations__"
    assert fresh.__annotations__ == {"format": 1}
    assert fresh.__annotations__ is bindings["__annotations__"]
    fresh.__annotations__ = {"ready": True}
    assert "__annotate__" not in bindings
    del fresh.__annotations__
    assert fresh.__annotations__ == {}

    child = TemplateModule("templates")
    assert type(child) is TemplateModule and isinstance(child, ModuleType)
    assert child.__class__ is TemplateModule and issubclass(TemplateModule, ModuleType)
    assert child.__doc__ == "template package" and child.label() == "templates"
    class FactoryModule(ModuleType):
        def __new__(cls, name):
            return fresh

        def __init__(self, name):
            raise AssertionError("foreign module must skip the subclass __init__")

    assert FactoryModule("unused") is fresh
    recursive = ModuleType("recursive")
    recursive.__file__ = recursive
    try:
        repr(recursive)
    except RecursionError:
        pass
    else:
        raise AssertionError("recursive module representation")

    class Later:
        def __repr__(self):
            raise AssertionError("looked beyond ModuleType.__repr__")

    class Plugin(ModuleType, Later):
        pass

    plugin = Plugin("billing")
    assert repr([plugin]) == "[<module 'billing'>]"

    from importlib.machinery import ModuleSpec

    class Origin:
        def __format__(self, fmt):
            plugin.__spec__.origin = "ready"
            return repr(plugin)

    plugin.__spec__ = ModuleSpec("billing", None, origin=Origin())
    assert repr(plugin) == "<module 'billing' (<module 'billing' (ready)>)>"

    reference = _weakref.ref(child)
    assert reference() is child
    del child
    assert reference() is None
    print("module identity, live namespace, subclass, weak reference")

    class Empty(ModuleType):
        __slots__ = ()

    class Fee(ModuleType):
        __slots__ = ("fee",)

    class Standard(Empty):
        __slots__ = ("price",)

    class Premium(Empty, Fee):
        __slots__ = ("price",)

    class Reordered(Fee, Empty):
        __slots__ = ("price",)

    assert Standard.__base__ is Empty
    assert Premium.__base__ is Reordered.__base__ is Fee
    members = vars(ModuleType)
    assert members["__new__"] is ModuleType.__new__
    assert ModuleType.__new__.__self__ is ModuleType
    assert members["__dict__"].__get__(plugin, ModuleType) is plugin.__dict__
    annotations = members["__annotations__"]
    annotations.__set__(plugin, {"price": int})
    assert annotations.__get__(plugin, ModuleType) is plugin.__annotations__
    annotations.__delete__(plugin)
    assert plugin.__annotations__ == {}

    quote = Standard("quote")
    quote.price = 42
    try:
        quote.__class__ = Premium
    except TypeError:
        pass
    else:
        raise AssertionError("accepted an additional inherited fee slot")
    assert type(quote) is Standard and quote.price == 42

    wide = Premium("wide")
    wide.price, wide.fee = 42, 13
    namespace = wide.__dict__
    ref = _weakref.ref(wide)
    for target in (Reordered, Premium):
        wide.__class__ = target
        assert type(wide) is target and wide.__dict__ is namespace and ref() is wide
        assert (wide.price, wide.fee) == (42, 13)
