from types import FunctionType


def current():
    def f(first=0, /, *rest, second=2, **extras):
        return first, rest, second, extras

    slot = FunctionType.__call__
    bound = f.__call__
    assert slot is type(f).__call__
    assert type(slot).__name__ == "wrapper_descriptor"
    assert type(bound).__name__ == "method-wrapper"
    assert slot.__objclass__ is FunctionType and bound.__self__ is f
    assert slot.__get__(None, FunctionType) is slot
    assert slot.__get__(f)() == bound() == f()
    slot_type = type(slot)
    get = slot_type.__get__
    assert not hasattr(slot_type, "__set__") and not hasattr(slot_type, "__delete__")
    assert type(get) is slot_type and get.__objclass__ is slot_type
    assert get(slot, None, FunctionType) is slot
    assert get(slot, f).__self__ is f and get(slot, f)() == f()
    assert get.__get__(slot).__self__ is slot
    expected = (1, (3,), 4, {"first": 9})
    assert slot(f, 1, 3, second=4, first=9) == expected
    assert bound(1, 3, second=4, first=9) == expected

    f.__call__ = None
    assert f.__call__ is None
    assert f() == bound() == slot(f)
    del f.__call__
    assert f.__call__.__self__ is f

    for invoke, message in (
        (lambda: get(list.append, f), "descriptor '__get__' requires a 'wrapper_descriptor' object but received a 'method_descriptor'"),
        (lambda: get.__get__(list.append), "descriptor '__get__' for 'wrapper_descriptor' objects doesn't apply to a 'method_descriptor' object"),
        (lambda: slot(), "descriptor '__call__' of 'function' object needs an argument"),
        (lambda: slot(self=f), "descriptor '__call__' of 'function' object needs an argument"),
        (lambda: slot(None), "descriptor '__call__' requires a 'function' object but received a 'NoneType'"),
        (lambda: slot.__get__(1), "descriptor '__call__' for 'function' objects doesn't apply to a 'int' object"),
    ):
        try:
            invoke()
        except TypeError as error:
            assert str(error) == message
        else:
            raise AssertionError("invalid function descriptor call accepted")
    print("function call descriptor")
