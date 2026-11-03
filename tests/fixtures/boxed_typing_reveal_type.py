import io
import sys
import typing
from typing import reveal_type


class Model:
    pass


class BrokenStream:
    def __init__(self, error):
        self.error = error

    def write(self, text):
        raise self.error


def current():
    assert typing.reveal_type is reveal_type
    param, = reveal_type.__type_params__
    assert param is not typing.T
    assert reveal_type.__annotations__ == {"obj": param, "return": param}

    alias = Model
    value = alias()
    saved = sys.stderr
    captured = io.StringIO()
    try:
        sys.stderr = captured
        assert reveal_type(value) is value
        assert reveal_type(alias) is Model
        try:
            reveal_type(obj=value)
        except TypeError:
            pass
        else:
            raise AssertionError("reveal_type accepted a keyword argument")
        assert captured.getvalue() == "Runtime type is 'Model'\nRuntime type is 'type'\n"

        error = RuntimeError("write failed")
        sys.stderr = BrokenStream(error)
        try:
            reveal_type(value)
        except RuntimeError as exc:
            assert exc is error
        else:
            raise AssertionError("reveal_type swallowed a stderr error")
    finally:
        sys.stderr = saved

    assert reveal_type(value) is value
