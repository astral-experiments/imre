from typing import *
import typing as _typing
from typing import __all__ as _exports

_public_names = [name for name in globals() if not name.startswith("_")]


def current():
    assert _exports is _typing.__all__
    assert _public_names == _exports
    for name in _exports:
        assert globals()[name] is getattr(_typing, name)
    print(_exports)

    assert _typing.evaluate_forward_ref(_typing.ForwardRef("int")) is int
    import doctest

    print([case.id() for case in doctest.DocTestSuite(_typing)])
