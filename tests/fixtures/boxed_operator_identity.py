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
    print("identity ok")
