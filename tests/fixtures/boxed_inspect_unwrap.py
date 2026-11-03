import inspect
import sys
from inspect import unwrap


def current():
    class Link:
        __hash__ = None

        def __init__(self, wrapped):
            self.__wrapped__ = wrapped

    def original():
        return 37

    inner = Link(original)
    outer = Link(inner)
    assert unwrap is inspect.unwrap
    assert unwrap(outer) is original
    assert unwrap(outer, stop=lambda x: x is inner) is inner

    class WrappedClass:
        __wrapped__ = original

    assert unwrap(WrappedClass) is WrappedClass
    inner.__wrapped__ = outer
    try:
        unwrap(outer)
    except ValueError as exc:
        assert "wrapper loop when unwrapping" in str(exc)
    else:
        raise AssertionError("unwrap accepted a cycle")
    assert unwrap(outer, stop=lambda x: x is inner) is inner
    saved_limit = sys.getrecursionlimit()
    try:
        sys.setrecursionlimit(80)
        chain = original
        for _ in range(sys.getrecursionlimit()):
            def layer():
                pass
            layer.__wrapped__ = chain
            chain = layer
        try:
            inspect.signature(chain)
        except ValueError as exc:
            assert "wrapper loop" in str(exc)
        else:
            raise AssertionError("signature exceeded the wrapper limit")
    finally:
        sys.setrecursionlimit(saved_limit)
    print(original())
