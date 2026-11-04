import functools


def current():
    placeholder = functools.Placeholder
    factory = type(placeholder)
    assert factory is functools._PlaceholderType
    try:
        functools.Placeholder = object()
        assert factory() is placeholder
    finally:
        functools.Placeholder = placeholder
    for args, kwargs in (((1,), {}), ((), {"value": 1})):
        try:
            factory(*args, **kwargs)
        except TypeError:
            pass
        else:
            raise AssertionError("Placeholder constructor accepted arguments")

    def record(*args, **keywords):
        return args, keywords

    class Binding(functools.partial):
        pass

    bound = Binding(record, factory(), 4, value=5)
    assert type(bound) is Binding and isinstance(bound, functools.partial)
    assert bound.func is record and bound.args == (placeholder, 4)
    assert bound.keywords == {"value": 5}
    assert bound(3, value=6) == ((3, 4), {"value": 6})
    bound.label = "bound"
    assert bound.__dict__ == {"label": "bound"}
    for attr in ("func", "args", "keywords"):
        try:
            setattr(bound, attr, None)
        except AttributeError:
            pass
        else:
            raise AssertionError(attr)

    class Custom(Binding):
        def __new__(cls, value):
            return super().__new__(cls, record, value)

        def __call__(self):
            return self.args[0]

    custom = Custom(9)
    assert type(custom) is Custom and custom() == 9

    for slot in ("__dict__", "__weakref__"):
        try:
            type("DuplicateSlot", (functools.partial,), {"__slots__": (slot,)})
        except TypeError as exc:
            assert slot in str(exc)
        else:
            raise AssertionError(slot)

    class Callable:
        def __call__(self, *args, **keywords):
            return args, keywords

    class Owner:
        basic = functools.partialmethod(record, 2, value=3)
        combined = functools.partialmethod(basic, 4, extra=5)
        static = functools.partialmethod(staticmethod(record), 2)
        cls = functools.partialmethod(classmethod(record), 2)
        over_partial = functools.partialmethod(functools.partial(record, extra=6), 3)
        callable = functools.partialmethod(Callable(), placeholder, 4)
        filled = functools.partialmethod(callable, 9)

    obj = Owner()
    assert obj.basic(4, value=5) == ((obj, 2, 4), {'value': 5})
    assert Owner.basic(obj, value=6) == ((obj, 2), {'value': 6})
    assert obj.combined(6) == ((obj, 2, 4, 6), {'value': 3, 'extra': 5})
    assert obj.basic.__self__ is obj
    assert Owner.cls.__self__ is Owner
    assert obj.cls(5) == ((Owner, 2, 5), {})
    assert obj.over_partial.__self__ is obj
    assert obj.over_partial(5) == ((obj, 3, 5), {'extra': 6})
    assert obj.static(5) == ((2, 5), {})
    assert obj.callable(1, 2) == ((obj, 1, 4, 2), {})
    assert obj.filled(7) == ((obj, 9, 4, 7), {})
    for invalid in (lambda: obj.callable(),
                    lambda: functools.partialmethod(None),
                    lambda: functools.partialmethod(record, placeholder),
                    lambda: functools.partialmethod(record, value=placeholder)):
        try:
            invalid()
        except TypeError:
            pass
        else:
            raise AssertionError("expected TypeError")
    wrapper = obj.basic
    assert wrapper.func.__func__ is record and wrapper.func.__self__ is obj
    assert wrapper.__module__ == "functools" and wrapper.__doc__ == functools.partial.__doc__
    wrapper.__doc__ = "instance doc"
    wrapper.__module__ = "instance_module"
    assert wrapper.__doc__ == "instance doc" and wrapper.__module__ == "instance_module"
    del wrapper.__module__, wrapper.__doc__
    assert wrapper.__module__ == "functools" and wrapper.__doc__ == functools.partial.__doc__

    class Target(Callable):
        def __repr__(self):
            return "target"

    p = Binding(Target(), 1, key=2)
    slot = functools.partial.__dict__["__repr__"]
    expected = Binding.__module__ + "." + Binding.__qualname__ + "(target, 1, key=2)"
    assert slot.__objclass__ is functools.partial
    assert repr(p) == slot.__get__(p, Binding)() == expected
    p.keywords["cycle"] = p
    assert repr(p).endswith("key=2, cycle=...)")
    for source, kind in [(wrapper, "functools.partial"), (p, "Binding")]:
        try:
            source.missing_marker
        except AttributeError as exc:
            assert str(exc) == f"'{kind}' object has no attribute 'missing_marker'"
        else:
            raise AssertionError(kind)

    # Native stack sizes differ. A finite chain must render or report ordinary recursion.
    nested = None
    for _ in range(512):
        nested = functools.partial(int, nested)
    try:
        rendered = repr(nested)
    except RecursionError:
        pass
    else:
        assert rendered.count("functools.partial(") == 512
    assert repr(p).endswith("key=2, cycle=...)")
    print("partialmethod bindings passed")
