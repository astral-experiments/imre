import inspect
import os
import sys


class Example:
    def method(self):
        pass


class Unloaded:
    __module__ = "_unloaded_inspect_test"


def current():
    module = sys.modules[__name__]
    assert inspect.getmodule(module) is module
    assert inspect.getmodule(Example) is module
    assert inspect.getmodule(Example.method) is module
    assert inspect.getmodule(Example.method.__code__) is module

    filename = module.__file__
    alias = os.path.join(os.path.dirname(filename), ".", os.path.basename(filename))
    assert inspect.getmodule(None, alias) is module
    assert inspect.getmodule(Unloaded, filename) is None

    # A cached filename resolves through the current sys.modules entry.
    sys.modules[__name__] = sys
    try:
        assert inspect.getmodule(None, filename) is sys
    finally:
        sys.modules[__name__] = module
