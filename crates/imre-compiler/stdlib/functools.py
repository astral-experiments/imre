"""Decorator implementations from CPython 3.14.5 Lib/functools.py (PSF-2.0)."""

from functools import Placeholder, WRAPPER_ASSIGNMENTS, WRAPPER_UPDATES, _CacheInfo, partial, reduce, update_wrapper
from types import GenericAlias


def wraps(wrapped,
          assigned = WRAPPER_ASSIGNMENTS,
          updated = WRAPPER_UPDATES):
    """Decorator factory to apply update_wrapper() to a wrapper function

       Returns a decorator that invokes update_wrapper() with the decorated
       function as the wrapper argument and the arguments to wraps() as the
       remaining arguments. Default arguments are as for update_wrapper().
       This is a convenience function to simplify applying partial() to
       update_wrapper().
    """
    return partial(update_wrapper, wrapped=wrapped,
                   assigned=assigned, updated=updated)


# The total ordering functions all invoke the root magic method directly
# rather than using the corresponding operator.  This avoids possible
# infinite recursion that could occur when the operator dispatch logic
# detects a NotImplemented result and then calls a reflected method.

def _gt_from_lt(self, other):
    'Return a > b.  Computed by @total_ordering from (not a < b) and (a != b).'
    op_result = type(self).__lt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result and self != other

def _le_from_lt(self, other):
    'Return a <= b.  Computed by @total_ordering from (a < b) or (a == b).'
    op_result = type(self).__lt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result or self == other

def _ge_from_lt(self, other):
    'Return a >= b.  Computed by @total_ordering from (not a < b).'
    op_result = type(self).__lt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result

def _ge_from_le(self, other):
    'Return a >= b.  Computed by @total_ordering from (not a <= b) or (a == b).'
    op_result = type(self).__le__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result or self == other

def _lt_from_le(self, other):
    'Return a < b.  Computed by @total_ordering from (a <= b) and (a != b).'
    op_result = type(self).__le__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result and self != other

def _gt_from_le(self, other):
    'Return a > b.  Computed by @total_ordering from (not a <= b).'
    op_result = type(self).__le__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result

def _lt_from_gt(self, other):
    'Return a < b.  Computed by @total_ordering from (not a > b) and (a != b).'
    op_result = type(self).__gt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result and self != other

def _ge_from_gt(self, other):
    'Return a >= b.  Computed by @total_ordering from (a > b) or (a == b).'
    op_result = type(self).__gt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result or self == other

def _le_from_gt(self, other):
    'Return a <= b.  Computed by @total_ordering from (not a > b).'
    op_result = type(self).__gt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result

def _le_from_ge(self, other):
    'Return a <= b.  Computed by @total_ordering from (not a >= b) or (a == b).'
    op_result = type(self).__ge__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result or self == other

def _gt_from_ge(self, other):
    'Return a > b.  Computed by @total_ordering from (a >= b) and (a != b).'
    op_result = type(self).__ge__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result and self != other

def _lt_from_ge(self, other):
    'Return a < b.  Computed by @total_ordering from (not a >= b).'
    op_result = type(self).__ge__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result

_convert = {
    '__lt__': [('__gt__', _gt_from_lt),
               ('__le__', _le_from_lt),
               ('__ge__', _ge_from_lt)],
    '__le__': [('__ge__', _ge_from_le),
               ('__lt__', _lt_from_le),
               ('__gt__', _gt_from_le)],
    '__gt__': [('__lt__', _lt_from_gt),
               ('__ge__', _ge_from_gt),
               ('__le__', _le_from_gt)],
    '__ge__': [('__le__', _le_from_ge),
               ('__gt__', _gt_from_ge),
               ('__lt__', _lt_from_ge)]
}

def total_ordering(cls):
    """Class decorator that fills in missing ordering methods"""
    # Find user-defined comparisons (not those inherited from object).
    roots = {op for op in _convert if getattr(cls, op, None) is not getattr(object, op, None)}
    if not roots:
        raise ValueError('must define at least one ordering operation: < > <= >=')
    root = max(roots)       # prefer __lt__ to __le__ to __gt__ to __ge__
    for opname, opfunc in _convert[root]:
        if opname not in roots:
            opfunc.__name__ = opname
            setattr(cls, opname, opfunc)
    return cls


def _lru_cache_decorator(maxsize, typed):
    from functools import _cache_wrapper, update_wrapper
    def decorating_function(user_function):
        wrapper = _cache_wrapper(user_function, maxsize, typed)
        wrapper.cache_parameters = lambda: {'maxsize': maxsize, 'typed': typed}
        return update_wrapper(wrapper, user_function)
    return decorating_function


def cache(user_function, /):
    'Simple lightweight unbounded cache.  Sometimes called "memoize".'
    from functools import lru_cache
    return lru_cache(maxsize=None)(user_function)


def _partial_prepare_merger(args):
    if not args:
        return 0, None
    nargs = len(args)
    order = []
    j = nargs
    for i, a in enumerate(args):
        if a is Placeholder:
            order.append(j)
            j += 1
        else:
            order.append(i)
    phcount = j - nargs
    if phcount:
        from operator import itemgetter
        return phcount, itemgetter(*order)
    return 0, None


class partialmethod:
    """Method descriptor with partial application of the given arguments
    and keywords.

    Supports wrapping existing descriptors and handles non-descriptor
    callables as instance methods.
    """

    def __new__(cls, func, /, *args, **keywords):
        # func could be a descriptor like classmethod which isn't callable
        if not callable(func) and not hasattr(func, "__get__"):
            raise TypeError(f"the first argument {func!r} must be a callable "
                            "or a descriptor")
        if args and args[-1] is Placeholder:
            raise TypeError("trailing Placeholders are not allowed")
        for value in keywords.values():
            if value is Placeholder:
                raise TypeError("Placeholder cannot be passed as a keyword argument")
        if isinstance(func, partialmethod):
            pto_phcount = func._phcount
            tot_args = func.args
            if args:
                tot_args += args
                if pto_phcount:
                    # Fill inner placeholders before appending leftover arguments.
                    nargs = len(args)
                    if nargs < pto_phcount:
                        tot_args += (Placeholder,) * (pto_phcount - nargs)
                    tot_args = func._merger(tot_args)
                    if nargs > pto_phcount:
                        tot_args += args[pto_phcount:]
                phcount, merger = _partial_prepare_merger(tot_args)
            else:
                phcount, merger = pto_phcount, func._merger
            keywords = {**func.keywords, **keywords}
            func = func.func
        else:
            tot_args = args
            phcount, merger = _partial_prepare_merger(tot_args)

        self = object.__new__(cls)
        self.func = func
        self.args = tot_args
        self.keywords = keywords
        self._phcount = phcount
        self._merger = merger
        return self

    def __repr__(self):
        cls = type(self)
        module = cls.__module__
        qualname = cls.__qualname__
        args = [repr(self.func)]
        args.extend(map(repr, self.args))
        args.extend(f"{k}={v!r}" for k, v in self.keywords.items())
        return f"{module}.{qualname}({', '.join(args)})"

    def _make_unbound_method(self):
        def _method(cls_or_self, /, *args, **keywords):
            phcount = self._phcount
            if phcount:
                try:
                    pto_args = self._merger(self.args + args)
                    args = args[phcount:]
                except IndexError:
                    raise TypeError("missing positional arguments "
                                    "in 'partialmethod' call; expected "
                                    f"at least {phcount}, got {len(args)}")
            else:
                pto_args = self.args
            keywords = {**self.keywords, **keywords}
            return self.func(cls_or_self, *pto_args, *args, **keywords)
        _method.__isabstractmethod__ = self.__isabstractmethod__
        _method.__partialmethod__ = self
        return _method

    def __get__(self, obj, cls=None):
        get = getattr(self.func, "__get__", None)
        if get is not None:
            new_func = get(obj, cls)
            if new_func is not self.func:
                result = partial(new_func, *self.args, **self.keywords)
                try:
                    result.__self__ = new_func.__self__
                except AttributeError:
                    pass
                return result
        return self._make_unbound_method().__get__(obj, cls)

    @property
    def __isabstractmethod__(self):
        return getattr(self.func, "__isabstractmethod__", False)

    __class_getitem__ = classmethod(GenericAlias)
