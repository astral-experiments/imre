import io
import sys
import typing
from typing import reveal_type
from weakref import ref


class Marker:
    pass


class Broken:
    def write(self, text):
        raise error


assert typing.reveal_type is reveal_type
param, = reveal_type.__type_params__
assert param is not typing.T and param.__infer_variance__
assert param.__name__ == "T" and param.__module__ == "typing"
assert param.evaluate_bound is param.evaluate_constraints is param.evaluate_default is None
assert ref(param)() is param
assert reveal_type.__annotations__ == {"obj": param, "return": param}
assert reveal_type.__annotate__(2) == reveal_type.__annotations__

value = Marker()
assert reveal_type(value) is value
assert typing.reveal_type(Marker) is Marker

real_stderr = sys.stderr
try:
    captured = io.StringIO()
    sys.stderr = captured
    assert typing.reveal_type(value) is value
    assert captured.getvalue() == "Runtime type is 'Marker'\n"
    try:
        reveal_type(obj=value)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a keyword")
    sys.stderr = Broken()
    error = RuntimeError("write failed")
    try:
        reveal_type(value)
    except RuntimeError as observed:
        assert observed is error
    else:
        raise AssertionError("ignored a failed write")
finally:
    sys.stderr = real_stderr
