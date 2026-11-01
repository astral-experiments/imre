def current():
    import sys
    from sys import _jit
    from types import ModuleType

    assert type(_jit) is ModuleType
    assert sys.__dict__["_jit"] is _jit
    assert repr(_jit) == "<module 'sys._jit'>"
    assert _jit.__spec__ is _jit.__loader__ is _jit.__package__ is None
    try:
        __import__("sys._jit")
    except ModuleNotFoundError:
        pass
    else:
        raise AssertionError("sys._jit unexpectedly importable")

    observed = []
    for name in ("is_available", "is_enabled", "is_active"):
        predicate = getattr(_jit, name)
        assert _jit.__dict__[name] is predicate
        assert predicate.__self__ is _jit
        assert predicate.__name__ == predicate.__qualname__ == name
        assert predicate.__module__ == "sys._jit"
        assert repr(predicate) == "<built-in function " + name + ">"
        assert type(predicate) is type(len)
        assert predicate.__text_signature__ == "($module, /)"
        print(predicate.__doc__)
        value = predicate()
        assert type(value) is bool
        observed.append(value)
        for call in (lambda: predicate(1, 2), lambda: predicate(x=1)):
            try:
                call()
            except TypeError as error:
                print(str(error))
            else:
                raise AssertionError("accepted arguments")

    available, enabled, active = observed
    assert not enabled or available
    assert not active or enabled
    original = _jit.is_enabled
    sys._jit = None
    try:
        del _jit.is_enabled
        assert original.__self__ is _jit and original() is enabled
    finally:
        _jit.is_enabled = original
        sys._jit = _jit
