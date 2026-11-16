import gc
import inspect
import types
import weakref
from functools import lru_cache


def current():
    calls = []

    def original(x: int, /, *, scale=2) -> int:
        """A documented function."""
        calls.append(x)
        return x * scale

    original.label = []
    cached = lru_cache()(original)
    outer = lru_cache()(cached)
    assert outer.__wrapped__ is cached and cached.__wrapped__ is original
    assert cached.__name__ == original.__name__
    assert cached.__qualname__ == original.__qualname__
    assert cached.__module__ == original.__module__
    assert cached.__doc__ == original.__doc__
    assert cached.__annotate__ is original.__annotate__
    assert cached.__type_params__ == original.__type_params__
    assert cached.__dict__ is not original.__dict__
    assert cached.label is original.label
    original.label = None
    assert cached.label == []
    assert cached.cache_parameters() == {"maxsize": 128, "typed": False}
    assert cached.cache_parameters() is not cached.cache_parameters()
    assert outer.cache_parameters is cached.cache_parameters
    for name, doc in (("cache_info", "Report cache statistics"),
                      ("cache_clear", "Clear the cache and cache statistics")):
        owner = type(cached)
        descriptor = vars(owner)[name]
        bound = getattr(cached, name)
        assert type(descriptor) is types.MethodDescriptorType and descriptor.__objclass__ is owner
        assert type(bound) is types.BuiltinMethodType and bound.__self__ is cached
        assert bound.__text_signature__ == descriptor.__text_signature__ == "($self, /)"
        assert bound.__doc__ == descriptor.__doc__ == doc
        assert bound.__qualname__ == descriptor.__qualname__ == "_lru_cache_wrapper." + name
        assert str(inspect.signature(descriptor)) == "(self, /)"
        assert str(inspect.signature(bound)) == "()"
        assert descriptor(cached) == bound()
        for invalid in (lambda: descriptor(), lambda: descriptor(2),
                        lambda: bound(1), lambda: bound(value=1)):
            try:
                invalid()
            except TypeError:
                pass
            else:
                raise AssertionError("invalid cache method call succeeded")
    other_parameters = lru_cache(original).cache_parameters
    assert type(lru_cache) is type(original)
    assert type(other_parameters) is type(original)
    assert other_parameters is not cached.cache_parameters
    assert other_parameters != cached.cache_parameters
    assert len({other_parameters, cached.cache_parameters, outer.cache_parameters}) == 2
    assert {cached.cache_parameters: "first", other_parameters: "second"}[outer.cache_parameters] == "first"
    for invalid in (lambda: cached.cache_parameters(1),
                    lambda: cached.cache_parameters(typed=True),
                    lambda: outer(x=3, scale=4)):
        try:
            invalid()
        except TypeError:
            pass
        else:
            raise AssertionError("invalid arguments accepted")
    assert outer(3, scale=4) == outer(3, scale=4) == 12
    assert calls == [3]

    def replacement(*args):
        raise AssertionError("metadata changed the cache's callable")

    cached.__wrapped__ = replacement
    assert cached(4, scale=3) == 12
    assert calls == [3, 4]
    del cached.__wrapped__
    assert not hasattr(cached, "__wrapped__")
    namespace = {"custom": 42}
    cached.__dict__ = namespace
    assert cached.__dict__ is namespace and cached.custom == 42
    cached.label = True
    assert namespace["label"] is True
    del cached.label
    assert "label" not in namespace
    for operation in (lambda: setattr(cached, "__dict__", []),
                      lambda: delattr(cached, "__dict__")):
        try:
            operation()
        except TypeError:
            pass
        else:
            raise AssertionError("invalid cache namespace operation succeeded")
    try:
        lru_cache()(42)
    except TypeError as error:
        assert str(error) == "the first argument must be callable"
    else:
        raise AssertionError("decorating a noncallable succeeded")

    class Example:
        @lru_cache
        def method(self, x):
            return x

    receiver = Example()
    method = receiver.method
    assert method.__self__ is receiver and method.__func__ is Example.method
    assert method.__wrapped__ is Example.method.__wrapped__
    assert method.__dict__ is Example.method.__dict__
    assert method.cache_parameters is Example.method.cache_parameters
    assert type(method).__name__ == "method"
    assert method(4) == Example.method.__wrapped__(receiver, 4) == 4
    assert lru_cache(method).__wrapped__ is method
    print(outer(3, scale=4), cached.custom, calls)

    wrapper = lru_cache(lambda: 7)
    ref, proxy = weakref.ref(wrapper), weakref.proxy(wrapper)
    assert ref() is wrapper and proxy() == 7
    del wrapper
    gc.collect()
    assert ref() is None
    try:
        proxy()
    except ReferenceError:
        pass
    else:
        raise AssertionError("dead cache wrapper proxy remained callable")
