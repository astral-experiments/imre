import gc


def current():
    effects = []

    def declare_scopes(name):
        class Example:
            name = 99
            value: (name, [name for _ in (0,)])

        def method(arg: ([0 for name in ()], lambda: name, lambda: name)):
            pass

        return Example, method.__annotate__

    example, provider = declare_scopes(7)
    assert example.__annotations__["value"] == (99, [7])
    first, second = provider(1)["arg"][1:]
    later = provider(1)["arg"][1]
    assert first.__closure__[0] is second.__closure__[0]
    assert first.__closure__[0] is not later.__closure__[0]
    first.__closure__[0].cell_contents = 119
    assert second() == 119
    try:
        later()
    except NameError as exc:
        assert exc.name == "name"
    else:
        raise AssertionError("a new provider invocation must have its own empty cell")

    def mark(value):
        effects.append(value)
        return value

    class Swapped:
        for which in (2, 1, 2):
            if which == 1:
                a: mark("first")
            if which == 2:
                b: mark("second")
            a: mark("last")
        # The executing class still owns the conditional set until its provider captures it.
        gc.collect()

    assert effects == []
    result = Swapped.__annotations__
    assert list(result) == ["a", "b"]
    assert result == {"a": "last", "b": "second"}
    assert Swapped.__annotations__ is result
    assert effects == ["first", "second", "last"]

    class Explicit:
        __annotations__ = 42
        x: mark("unused")

    manual = {}
    class Replaced:
        x: mark("never")
        __annotations__ = manual
        y: mark("either")

    assert Explicit.__annotations__ == 42
    assert Replaced.__annotations__ is manual and manual == {}
    assert effects == ["first", "second", "last"]

    ready = []
    def later():
        if not ready:
            raise ValueError("deferred")
        return bool

    class Retry:
        x: mark("retry")
        x: later()

    try:
        Retry.__annotations__
    except ValueError as exc:
        assert exc.args == ("deferred",)
    else:
        raise AssertionError("first evaluation should fail")
    ready.append(True)
    assert Retry.__annotations__ == {"x": bool}
    assert effects[-2:] == ["retry", "retry"]

    def replace():
        Reentrant.__annotations__ = {"temporary": 0}
        gc.collect()
        return int

    class Reentrant:
        x: replace()
        y: list

    assert Reentrant.__annotations__ == {"x": int, "y": list}
    class ManualProvider:
        def __annotate__(format):
            return {"field": format}

    class Child(ManualProvider):
        pass

    assert "__annotations__" not in vars(Child)
    assert "__annotate_func__" not in vars(Child)
    assert Child.__annotate__ is None
    assert Child.__annotations__ == {}
    assert ManualProvider.__annotations__ == {"field": 1}
    del ManualProvider.__annotations__
    assert ManualProvider.__annotate__ is None

    class Meta(type):
        def __annotate__(cls, format):
            return {"owner": cls}

    class Custom(metaclass=Meta):
        pass

    assert Custom.__annotations__ == {"owner": Custom}
    Custom.__annotate__ = 42
    assert vars(Custom)["__annotate__"] == 42
    del Custom.__annotate__
    assert Custom.__annotate__(1) == {"owner": Custom}
    try:
        del Custom.__annotate__
    except AttributeError as exc:
        assert str(exc) == "type object 'Custom' has no attribute '__annotate__'"
    else:
        raise AssertionError("deleted the metaclass provider")

    ns = {"Parent": ManualProvider, "__name__": "class_sites"}
    exec("from __future__ import annotations\nclass Empty(Parent): pass\nclass Strings(Parent): x: Missing", ns)
    assert ns["Empty"].__annotate__ is None
    assert ns["Empty"].__annotations__ == {}
    assert ns["Strings"].__annotations__ == {"x": "Missing"}

    class NewMeta(type):
        def __new__(mcls, name, bases, namespace):
            if name == "Unreached":
                assert namespace["__annotations__"] == {}
            return super().__new__(mcls, name, bases, namespace)

    class CustomParent(metaclass=NewMeta):
        def __annotate__(format):
            return {"value": int}

    class CustomChild(CustomParent):
        pass

    assert CustomParent.__annotations__ == {"value": int}
    assert CustomChild.__annotate__ is None
    assert CustomChild.__annotations__ == {}
    ns = {"NewMeta": NewMeta}
    exec("from __future__ import annotations\nclass Unreached(metaclass=NewMeta):\n if False: field: int", ns)
    assert ns["Unreached"].__annotations__ == {}

    print("class annotation sites")

    from types import FunctionType

    def declare(outer, conditional):
        class Example:
            alias = bytes
            first: alias
            __private: __class__
            if conditional:
                optional: outer
            last: NotDefined
        return Example

    for conditional in (False, True):
        cls = declare(int, conditional)
        annotate = cls.__annotate__
        cells = dict(zip(annotate.__code__.co_freevars, annotate.__closure__))
        assert cells["__class__"].cell_contents is cls
        try:
            cls.__annotations__
        except NameError as error:
            assert error.name == "NotDefined"
        else:
            raise AssertionError("reading unresolved annotations succeeded")
        expected = {"first": bytes, "_Example__private": cls}
        if conditional:
            expected["optional"] = int
        expected["last"] = str
        # A caller supplies synthetic globals for VALUE_WITH_FAKE_GLOBALS.
        replay = FunctionType(annotate.__code__, {"NotDefined": str}, closure=annotate.__closure__)
        assert replay(2) == expected
        assert "NotDefined" not in globals()
