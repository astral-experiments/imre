import gc


def current():
    effects = []

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
