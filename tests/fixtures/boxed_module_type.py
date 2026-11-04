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

    # Name lookup follows live dictionary entries after collisions, deletion and growth.
    class Collision:
        def __hash__(self):
            return hash("needle")

        def __eq__(self, other):
            return False

    names = ModuleType("names")
    entries = names.__dict__
    collision = Collision()
    entries[collision] = "other"
    entries["needle"] = "first"
    assert names.needle == "first"
    del entries[collision]
    for i in range(75):
        entries[str(i)] = i
    for i in range(0, 75, 2):
        del entries[str(i)]
    for name in ("needle", "café", "𐐀"):
        entries[name] = name
        assert getattr(names, name) == name
        del entries[name]
        assert not hasattr(names, name)
        entries[name] = "new"
        assert getattr(names, name) == "new"

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

    reference = _weakref.ref(child)
    assert reference() is child
    del child
    assert reference() is None
    print("module identity, live namespace, subclass, weak reference")
