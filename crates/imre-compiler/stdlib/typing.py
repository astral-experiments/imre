"""Stream annotations and special forms for the runtime typing module."""

import typing as _typing
from functools import lru_cache
from typing import Generic, TypeVar


T = TypeVar("T")
KT = TypeVar("KT")
VT = TypeVar("VT")
AnyStr = TypeVar("AnyStr", bytes, str)


class IO(Generic[AnyStr]):
    __slots__ = ()


class BinaryIO(IO[bytes]):
    __slots__ = ()


class TextIO(IO[str]):
    __slots__ = ()


# Runtime assertion from CPython 3.14.5 Lib/typing.py. See stdlib/docstrings/LICENSE.
_ASSERT_NEVER_REPR_MAX_LENGTH = 100


def assert_never(arg, /):
    """Statically assert that a line of code is unreachable."""
    value = repr(arg)
    if len(value) > _ASSERT_NEVER_REPR_MAX_LENGTH:
        value = value[:_ASSERT_NEVER_REPR_MAX_LENGTH] + "..."
    raise AssertionError(f"Expected code to be unreachable, but got: {value}")


# Optional and the special form protocol from CPython 3.14.5 Lib/typing.py.
# See stdlib/docstrings/LICENSE. Union and the type parameters are native exports.
class _Final:
    __module__ = __name__
    __slots__ = ("__weakref__",)


class _NotIterable:
    __module__ = __name__
    __slots__ = ()
    __iter__ = None


class _SpecialForm(_Final, _NotIterable):
    __module__ = __name__
    __slots__ = ("_name", "__doc__", "_getitem")

    def __init__(self, getitem):
        self._getitem = getitem
        self._name = getitem.__name__
        self.__doc__ = getitem.__doc__

    def __getattr__(self, name):
        if name in ("__name__", "__qualname__"):
            return self._name
        raise AttributeError(name)

    def __mro_entries__(self, bases):
        raise TypeError(f"Cannot subclass {repr(self)}")

    def __repr__(self):
        return "typing." + self._name

    def __reduce__(self):
        return self._name

    def __call__(self, *args, **kwds):
        raise TypeError(f"Cannot instantiate {repr(self)}")

    def __or__(self, other):
        return _typing.Union[self, other]

    def __ror__(self, other):
        return _typing.Union[other, self]

    def __instancecheck__(self, obj):
        raise TypeError(f"{repr(self)} cannot be used with isinstance()")

    def __subclasscheck__(self, cls):
        raise TypeError(f"{repr(self)} cannot be used with issubclass()")

    @lru_cache
    def _cached_getitem(self, parameters):
        return self._getitem(self, parameters)

    def __getitem__(self, parameters):
        try:
            return self._cached_getitem(parameters)
        except TypeError:
            # Like _tp_cache, retry uncached for unhashable arguments.
            return self._getitem(self, parameters)


@_SpecialForm
def NoReturn(self, parameters):
    """Special type indicating functions that never return.

    Example::

        from typing import NoReturn

        def stop() -> NoReturn:
            raise Exception('no way')

    NoReturn can also be used as a bottom type, a type that
    has no values. Starting in Python 3.11, the Never type should
    be used for this concept instead. Type checkers should treat the two
    equivalently.
    """
    raise TypeError(f"{self} is not subscriptable")


@_SpecialForm
def Never(self, parameters):
    """The bottom type, a type that has no members.

    This can be used to define a function that should never be
    called, or a function that never returns::

        from typing import Never

        def never_call_me(arg: Never) -> None:
            pass

        def int_or_str(arg: int | str) -> None:
            never_call_me(arg)  # type checker error
            match arg:
                case int():
                    print("It's an int")
                case str():
                    print("It's a str")
                case _:
                    never_call_me(arg)  # OK, arg is of type Never
    """
    raise TypeError(f"{self} is not subscriptable")


@_SpecialForm
def Self(self, parameters):
    """Used to spell the type of "self" in classes.

    Example::

        from typing import Self

        class Foo:
            def return_self(self) -> Self:
                ...
                return self

    This is especially useful for:
        - classmethods that are used as alternative constructors
        - annotating an `__enter__` method which returns self
    """
    raise TypeError(f"{self} is not subscriptable")


@_SpecialForm
def LiteralString(self, parameters):
    """Represents an arbitrary literal string.

    Example::

        from typing import LiteralString

        def run_query(sql: LiteralString) -> None:
            ...

        def caller(arbitrary_string: str, literal_string: LiteralString) -> None:
            run_query("SELECT * FROM students")  # OK
            run_query(literal_string)  # OK
            run_query("SELECT * FROM " + literal_string)  # OK
            run_query(arbitrary_string)  # type checker error
            run_query(  # type checker error
                f"SELECT * FROM students WHERE name = {arbitrary_string}"
            )

    Only string literals and other LiteralStrings are compatible
    with LiteralString. This provides a tool to help prevent
    security issues such as SQL injection.
    """
    raise TypeError(f"{self} is not subscriptable")


@_SpecialForm
def TypeAlias(self, parameters):
    """Special form for marking type aliases.

    Use TypeAlias to indicate that an assignment should
    be recognized as a proper type alias definition by type
    checkers.

    For example::

        Predicate: TypeAlias = Callable[..., bool]

    It's invalid when used anywhere except as in the example above.
    """
    raise TypeError(f"{self} is not subscriptable")


def _type_check(arg, message, *, allow_special_forms=False):
    protocol = getattr(_typing, "Protocol", None)
    if isinstance(arg, _SpecialForm) and arg in (LiteralString, NoReturn, Never, Self, TypeAlias):
        return arg
    if allow_special_forms and (arg is ClassVar or arg is Final):
        return arg
    if isinstance(arg, _SpecialForm) or arg is _typing.Generic or (protocol is not None and arg is protocol):
        raise TypeError(f"Plain {repr(arg)} is not valid as type argument")
    origin = _typing.get_origin(arg)
    if (
        origin is _typing.Generic
        or (protocol is not None and origin is protocol)
        or (not allow_special_forms and (origin is ClassVar or origin is Final))
    ):
        raise TypeError(f"{repr(arg)} is not valid as type argument")
    if type(arg) is tuple:
        raise TypeError(f"{message} Got {repr(arg)[:100]}.")
    return arg


@_SpecialForm
def ClassVar(self, parameters):
    """Special type construct to mark class variables."""
    item = _type_check(parameters, f"{self} accepts only single type.", allow_special_forms=True)
    if isinstance(item, str):
        raise NotImplementedError("string ClassVar arguments require typing.ForwardRef")
    return _typing._GenericAlias(self, (type(None) if item is None else item,))


@_SpecialForm
def Final(self, parameters):
    """Special typing construct to indicate final names to type checkers."""
    item = _type_check(parameters, f"{self} accepts only single type.", allow_special_forms=True)
    if isinstance(item, str):
        raise NotImplementedError("string Final arguments require typing.ForwardRef")
    return _typing._GenericAlias(self, (type(None) if item is None else item,))


@_SpecialForm
def Unpack(self, parameters):
    """Unpack the types in a tuple or a variadic type parameter."""
    item = _type_check(parameters, f"{self} accepts only single type.")
    if isinstance(item, str):
        raise NotImplementedError("string Unpack arguments require typing.ForwardRef")
    return _typing._UnpackGenericAlias(self, (type(None) if item is None else item,))


@_SpecialForm
def Optional(self, parameters):
    """Optional[X] is equivalent to Union[X, None]."""
    arg = _type_check(parameters, f"{repr(self)} requires a single type.")
    return _typing.Union[arg, type(None)]


def _alias_type_arg(arg, message="Parameters to generic types must be types."):
    arg = _type_check(arg, message)
    if isinstance(arg, str):
        raise NotImplementedError("string alias arguments require typing.ForwardRef")
    return type(None) if arg is None else arg


def _is_parameterized_typing_alias(value):
    return type(value) in (_typing._GenericAlias, _typing._CallableGenericAlias, _typing._LiteralGenericAlias)


def _named_alias_bases(alias, bases):
    # As in _BaseGenericAlias.__mro_entries__, contribute Generic only if a
    # later base will not do so. User-provided base hooks receive the original tuple.
    result = [] if alias.__origin__ in bases else [alias.__origin__]
    for index, base in enumerate(bases):
        if base is alias or base == alias:
            break
    else:
        raise ValueError("tuple.index(x): x not in tuple")
    for base in bases[index + 1:]:
        if isinstance(base, _SpecialGenericAlias) or _is_parameterized_typing_alias(base) or base is _typing.Callable:
            break
        if isinstance(base, type):
            if issubclass(base, Generic):
                break
        else:
            hook = getattr(base, "__mro_entries__", None)
            expanded = hook(bases) if hook else None
            if isinstance(expanded, tuple) and any(isinstance(b, type) and issubclass(b, Generic) for b in expanded):
                break
    else:
        result.append(Generic)
    return tuple(result)


class _SpecialGenericAlias(_NotIterable):
    """A named typing alias for a builtin collection."""
    __module__ = __name__

    def __init__(self, origin, nparams, *, name):
        self.__origin__ = origin
        self._name = name
        self.__qualname__ = name
        self._inst = False
        self._nparams = nparams
        self.__slots__ = None
        self.__doc__ = f"Deprecated alias to {origin.__qualname__}."

    def __repr__(self):
        return "typing." + self._name

    def __reduce__(self):
        return self._name

    def __getattr__(self, attr):
        if attr in ("__name__", "__qualname__"):
            return self._name
        if not (attr.startswith("__") and attr.endswith("__")):
            return getattr(self.__origin__, attr)
        raise AttributeError(attr)

    def __setattr__(self, attr, value):
        if (attr.startswith("__") and attr.endswith("__")) or attr in ("_name", "_inst", "_nparams"):
            object.__setattr__(self, attr, value)
        else:
            setattr(self.__origin__, attr, value)

    def __call__(self, *args, **kwargs):
        if not self._inst:
            raise TypeError(f"Type {self._name} cannot be instantiated; use {self.__origin__.__name__}() instead")
        result = self.__origin__(*args, **kwargs)
        try:
            result.__orig_class__ = self
        except Exception:
            pass
        return result

    def _getitem(self, params):
        if not isinstance(params, tuple):
            params = (params,)
        args = tuple(_alias_type_arg(p) for p in params)
        if len(args) != self._nparams:
            count = "many" if len(args) > self._nparams else "few"
            raise TypeError(f"Too {count} arguments for {self}; actual {len(args)}, expected {self._nparams}")
        return self.copy_with(args)

    @lru_cache
    def _cached_getitem(self, params):
        return self._getitem(params)

    def __getitem__(self, params):
        try:
            return self._cached_getitem(params)
        except TypeError:
            return self._getitem(params)

    def copy_with(self, params):
        return _typing._GenericAlias(self.__origin__, params, name=self._name, inst=self._inst)

    def __instancecheck__(self, value):
        return self.__subclasscheck__(type(value))

    def __subclasscheck__(self, cls):
        if isinstance(cls, _SpecialGenericAlias):
            cls = cls.__origin__
        elif cls is _typing.Callable:
            cls = _typing.get_origin(cls)
        elif _is_parameterized_typing_alias(cls):
            raise TypeError("Subscripted generics cannot be used with class and instance checks")
        return issubclass(cls, self.__origin__)

    def __mro_entries__(self, bases):
        return _named_alias_bases(self, bases)

    def __or__(self, other):
        return _typing.Union[self, other]

    def __ror__(self, other):
        return _typing.Union[other, self]


class _TupleType(_SpecialGenericAlias):
    __module__ = __name__

    def _getitem(self, params):
        if not isinstance(params, tuple):
            params = (params,)
        message = "Tuple[t0, t1, ...]: each t must be a type."
        if len(params) >= 2 and params[-1] is ...:
            message = "Tuple[t, ...]: t must be a type."
        return self.copy_with(tuple(_alias_type_arg(p, message) for p in params))


Tuple = _TupleType(tuple, -1, name="Tuple")
Tuple.__doc__ = \
    """Deprecated alias to builtins.tuple.

    Tuple[X, Y] is the cross-product type of X and Y.

    Example: Tuple[T1, T2] is a tuple of two elements corresponding
    to type variables T1 and T2.  Tuple[int, float, str] is a tuple
    of an int, a float and a string.

    To specify a variable-length tuple of homogeneous type, use Tuple[T, ...].
    """
List = _SpecialGenericAlias(list, 1, name="List")


def _substitute_alias_argument(arg, replacements):
    for parameter, value in replacements.items():
        if arg is parameter:
            return value
    parameters = getattr(arg, '__parameters__', ())
    if not parameters:
        return arg
    from types import GenericAlias
    if type(arg) is GenericAlias:
        return GenericAlias(arg.__origin__, tuple(
            _substitute_alias_argument(item, replacements) for item in arg.__args__
        ))
    return arg[tuple(replacements[p] for p in parameters)]


def _specialize_named_alias_uncached(alias, args):
    parameters = alias.__parameters__
    if not parameters:
        raise TypeError(f"{alias} is not a generic class")
    if not isinstance(args, tuple):
        args = (args,)
    if alias._name == "Tuple" and any(isinstance(p, _typing.TypeVarTuple) for p in parameters):
        raise NotImplementedError("specializing a variadic typing.Tuple requires parameter-pack substitution")
    args = tuple(_alias_type_arg(arg) for arg in args)
    if len(args) != len(parameters):
        which = "few" if len(args) < len(parameters) else "many"
        expected = f"at least {len(parameters)}" if which == "few" else str(len(parameters))
        raise TypeError(f"Too {which} arguments for {alias}; actual {len(args)}, expected {expected}")
    replacements = dict(zip(parameters, args))
    args = tuple(_substitute_alias_argument(arg, replacements) for arg in alias.__args__)
    return _typing._GenericAlias(alias.__origin__, args, name=alias._name, inst=alias._inst)


_cached_named_specialization = lru_cache(_specialize_named_alias_uncached)


def _specialize_named_alias(alias, args):
    try:
        return _cached_named_specialization(alias, args)
    except TypeError:
        return _specialize_named_alias_uncached(alias, args)


class _TypedCacheSpecialForm(_SpecialForm):
    __module__ = __name__

    @lru_cache
    def _typed_getitem(self, parameters, types):
        # As in _tp_cache(typed=True), only the immediate argument types are keys.
        return self._getitem(self, *parameters)

    def __getitem__(self, parameters):
        if not isinstance(parameters, tuple):
            parameters = (parameters,)
        try:
            return self._typed_getitem(parameters, tuple(type(p) for p in parameters))
        except TypeError:
            return self._getitem(self, *parameters)


@_TypedCacheSpecialForm
def Literal(self, *parameters):
    """A type whose arguments are literal values."""
    return _typing._LiteralGenericAlias(self, parameters)
