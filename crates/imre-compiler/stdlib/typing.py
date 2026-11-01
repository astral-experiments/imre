"""Stream annotations and special forms for the runtime typing module."""

import typing as _typing
from functools import lru_cache
from typing import Generic, TypeVar


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
    if allow_special_forms and arg is Final:
        return arg
    if isinstance(arg, _SpecialForm) or arg is _typing.Generic or (protocol is not None and arg is protocol):
        raise TypeError(f"Plain {repr(arg)} is not valid as type argument")
    origin = _typing.get_origin(arg)
    if (
        origin is _typing.Generic
        or (protocol is not None and origin is protocol)
        or (not allow_special_forms and origin is Final)
    ):
        raise TypeError(f"{repr(arg)} is not valid as type argument")
    if type(arg) is tuple:
        raise TypeError(f"{message} Got {repr(arg)[:100]}.")
    return arg


@_SpecialForm
def Final(self, parameters):
    """Special typing construct to indicate final names to type checkers."""
    item = _type_check(parameters, f"{self} accepts only single type.", allow_special_forms=True)
    if isinstance(item, str):
        raise NotImplementedError("string Final arguments require typing.ForwardRef")
    return _typing._GenericAlias(self, (type(None) if item is None else item,))


@_SpecialForm
def Optional(self, parameters):
    """Optional[X] is equivalent to Union[X, None]."""
    arg = _type_check(parameters, f"{repr(self)} requires a single type.")
    return _typing.Union[arg, type(None)]


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
