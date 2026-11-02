# CPython 3.14.5 Lib/reprlib.py. See ../docstrings/LICENSE.
from _thread import get_ident as _get_ident

def _recursive_repr(fillvalue='...'):
    'Decorator to make a repr function return fillvalue for a recursive call'

    def decorating_function(user_function):
        repr_running = set()

        def wrapper(self):
            key = id(self), _get_ident()
            if key in repr_running:
                return fillvalue
            repr_running.add(key)
            try:
                result = user_function(self)
            finally:
                repr_running.discard(key)
            return result

        # Can't use functools.wraps() here because of bootstrap issues
        wrapper.__module__ = getattr(user_function, '__module__')
        wrapper.__doc__ = getattr(user_function, '__doc__')
        wrapper.__name__ = getattr(user_function, '__name__')
        wrapper.__qualname__ = getattr(user_function, '__qualname__')
        wrapper.__annotate__ = getattr(user_function, '__annotate__', None)
        wrapper.__type_params__ = getattr(user_function, '__type_params__', ())
        wrapper.__wrapped__ = user_function
        return wrapper

    return decorating_function
