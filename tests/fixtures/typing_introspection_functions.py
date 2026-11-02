import inspect
import types
import typing
from typing import get_args, get_origin


for fn, name in ((get_args, "get_args"), (get_origin, "get_origin")):
    assert fn is getattr(typing, name)
    assert type(fn) is types.FunctionType
    assert fn.__module__ == "typing" and fn.__name__ == fn.__qualname__ == name
    assert str(inspect.signature(fn)) == "(tp)"
    assert fn.__doc__.startswith("Get ") and ">>>" in fn.__doc__
    for args, kwargs in [((), {}), ((int, str), {}), ((), {"value": int})]:
        try:
            fn(*args, **kwargs)
        except TypeError:
            pass
        else:
            raise AssertionError("invalid arguments accepted")

value = list[int]
assert get_origin(tp=value) is list and get_args(tp=value) == (int,)
assert typing.get_origin(value) is list and typing.get_args(value) == (int,)
assert get_origin(object()) is None and get_args(object()) == ()
