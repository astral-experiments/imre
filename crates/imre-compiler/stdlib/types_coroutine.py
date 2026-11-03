# Adapted from CPython 3.14.5 Lib/types.py. See stdlib/docstrings/LICENSE.
from types import CodeType, CoroutineType, FunctionType, GeneratorType, _coroutine_flags, _mark_generator


def coroutine(func):
    """Convert regular generator function to a coroutine."""
    if not callable(func):
        raise TypeError('types.coroutine() expects a callable')

    if (func.__class__ is FunctionType and
            getattr(func, '__code__', None).__class__ is CodeType):
        flags = _coroutine_flags(func.__code__)
        if flags & 0x180:
            return func
        if flags & 0x20:
            code = func.__code__
            if type(code) is CodeType:
                _mark_generator(func, code)
            else:
                func.__code__ = code.replace(co_flags=code.co_flags | 0x100)
            return func

    import functools
    import _collections_abc

    @functools.wraps(func)
    def wrapped(*args, **kwargs):
        coro = func(*args, **kwargs)
        if (coro.__class__ is CoroutineType or
                coro.__class__ is GeneratorType and _coroutine_flags(coro.gi_code) & 0x100):
            return coro
        if (isinstance(coro, _collections_abc.Generator) and
                not isinstance(coro, _collections_abc.Coroutine)):
            return _GeneratorWrapper(coro)
        return coro

    return wrapped
