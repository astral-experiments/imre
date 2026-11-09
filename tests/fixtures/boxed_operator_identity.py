def current():
    import _operator
    import inspect
    import operator

    class Guard:
        def __eq__(self, other):
            raise AssertionError("identity called __eq__")

        def __ne__(self, other):
            raise AssertionError("identity called __ne__")

    a = Guard()
    b = Guard()
    assert operator.is_(a, a)
    assert operator.is_not(a, b)
    assert not operator.is_([1], [1])
    assert operator.is_not(True, 1)
    assert operator.is_(None, None)
    for name in ("is_", "is_not"):
        function = getattr(operator, name)
        assert function is getattr(_operator, name)
        assert str(inspect.signature(function)) == "(a, b, /)"
    assert not hasattr(operator, "__is__")
    assert not hasattr(operator, "__is_not__")
    def capture(*args, **kwargs):
        return args, kwargs

    class Key(str):
        pass

    key = Key("obj")
    assert operator.__call__ is _operator.call is operator.call
    assert str(inspect.signature(operator.call)) == "(obj, /, *args, **kwargs)"
    args, kwargs = operator.call(capture, 7, **{key: 3})
    assert args == (7,) and next(iter(kwargs)) is key and kwargs[key] == 3
    assert operator.call(locals)["key"] is key
    print("identity ok")
