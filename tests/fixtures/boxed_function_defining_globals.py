def current():
    seen = []
    failure = RuntimeError("lookup")

    class Key(str):
        __hash__ = str.__hash__

        def __eq__(self, other):
            seen.append(other)
            if len(seen) == 3:
                raise failure
            return super().__eq__(other)

    namespace = globals()
    name_key = Key("__name__")
    builtins_key = Key("__builtins__")
    module = object()
    captured = {"token": object()}
    original_name = namespace.pop("__name__")
    original = namespace.pop("__builtins__")
    try:
        namespace[name_key] = module
        namespace[builtins_key] = captured

        def saved():
            return token

        try:
            def saved():
                return None
        except RuntimeError as exc:
            assert exc is failure
        else:
            raise AssertionError("second definition completed")
    finally:
        del namespace[name_key]
        del namespace[builtins_key]
        namespace["__name__"] = original_name
        namespace["__builtins__"] = original
    assert seen == ["__name__", "__builtins__", "__name__"]
    assert saved.__module__ is module
    assert saved.__globals__ is namespace and saved.__builtins__ is captured
    assert saved() is captured["token"]

    from types import FunctionType

    class Globals(dict):
        def __getitem__(self, key):
            raise AssertionError("function construction subscribed to globals")

    namespace = Globals({name_key: module, builtins_key: captured})
    seen.clear()
    copied = FunctionType(saved.__code__, namespace)
    assert seen == ["__name__", "__builtins__"]
    assert copied.__module__ is module
    assert copied.__globals__ is namespace and copied.__builtins__ is captured
    try:
        copied = FunctionType(saved.__code__, namespace)
    except RuntimeError as exc:
        assert exc is failure
    else:
        raise AssertionError("second construction completed")
    assert seen == ["__name__", "__builtins__", "__name__"]
    assert copied.__module__ is module
