"""Stream annotations and special forms for the runtime typing module."""

import _typing as _typing_runtime
import builtins as _builtins
import functools
import sys as _sys
import typing as _typing
import _collections_abc as _abc
from _collections_abc import Generator as _Generator, Iterable as _Iterable
from _collections_abc import Mapping as _Mapping, MutableMapping as _MutableMapping, Sequence as _Sequence
from _collections_abc import Reversible as _Reversible, Sized as _Sized
from collections.abc import ByteString as _ByteString
from collections import Counter as _Counter, defaultdict as _defaultdict
from collections import ChainMap as _ChainMap, OrderedDict as _OrderedDict, deque as _deque
from collections.abc import Callable as _ABCCallable
from _collections_abc import AsyncIterator as _AsyncIterator, Awaitable as _Awaitable
from abc import abstractmethod
from annotationlib import ForwardRef, _make_forward_ref
from functools import lru_cache
from _typing import _idfunc
from typing import Any, Callable, Generic, Protocol, TypeVar, Union
from typing import ParamSpec, ParamSpecArgs, ParamSpecKwargs, TypeVarTuple


# From CPython 3.14.5 Lib/typing.py. See stdlib/docstrings/LICENSE.
def _is_unpacked_typevartuple(x: Any) -> bool:
    # Need to check 'is True' here
    # See: https://github.com/python/cpython/issues/137706
    return ((not isinstance(x, type)) and
            getattr(x, '__typing_is_unpacked_typevartuple__', False) is True)


def _collect_type_parameters(args, *, enforce_default_ordering: bool = True):
    """Collect all type parameters in args
    in order of first appearance (lexicographic order).

    For example::

        >>> P = ParamSpec('P')
        >>> T = TypeVar('T')
        >>> _collect_type_parameters((T, Callable[P, T]))
        (~T, ~P)
    """
    # required type parameter cannot appear after parameter with default
    default_encountered = False
    # or after TypeVarTuple
    type_var_tuple_encountered = False
    parameters = []
    for t in args:
        if isinstance(t, type):
            # We don't want __parameters__ descriptor of a bare Python class.
            pass
        elif isinstance(t, tuple):
            # `t` might be a tuple, when `ParamSpec` is substituted with
            # `[T, int]`, or `[int, *Ts]`, etc.
            for x in t:
                for collected in _collect_type_parameters([x]):
                    if collected not in parameters:
                        parameters.append(collected)
        # Native TypeVar and TypeVarTuple have substitution slots but do not expose
        # their bound __typing_subst__ methods to Python yet.
        elif isinstance(t, (TypeVar, TypeVarTuple)) or hasattr(t, '__typing_subst__'):
            if t not in parameters:
                if enforce_default_ordering:
                    if type_var_tuple_encountered and t.has_default():
                        raise TypeError('Type parameter with a default'
                                        ' follows TypeVarTuple')

                    if t.has_default():
                        default_encountered = True
                    elif default_encountered:
                        raise TypeError(f'Type parameter {t!r} without a default'
                                        ' follows type parameter with a default')

                parameters.append(t)
        else:
            if _is_unpacked_typevartuple(t):
                type_var_tuple_encountered = True
            for x in getattr(t, '__parameters__', ()):
                if x not in parameters:
                    parameters.append(x)
    return tuple(parameters)


def _caller(depth=1, default='__main__'):
    try:
        return _sys._getframemodulename(depth + 1) or default
    except AttributeError:  # For platforms without _getframemodulename()
        pass
    try:
        return _sys._getframe(depth + 1).f_globals.get('__name__', default)
    except (AttributeError, ValueError):  # For platforms without _getframe()
        pass
    return None

class NewType:
    """NewType creates simple unique types with almost zero runtime overhead.

    NewType(name, tp) is considered a subtype of tp
    by static type checkers. At runtime, NewType(name, tp) returns
    a dummy callable that simply returns its argument.

    Usage::

        UserId = NewType('UserId', int)

        def name_by_id(user_id: UserId) -> str:
            ...

        UserId('user')          # Fails type check

        name_by_id(42)          # Fails type check
        name_by_id(UserId(42))  # OK

        num = UserId(5) + 1     # type: int
    """

    __call__ = _idfunc

    def __init__(self, name, tp):
        self.__qualname__ = name
        if '.' in name:
            name = name.rpartition('.')[-1]
        self.__name__ = name
        self.__supertype__ = tp
        def_mod = _caller()
        if def_mod != 'typing':
            self.__module__ = def_mod

    def __mro_entries__(self, bases):
        # We defined __mro_entries__ to get a better error message
        # if a user attempts to subclass a NewType instance. bpo-46170
        superclass_name = self.__name__

        class Dummy:
            def __init_subclass__(cls):
                subclass_name = cls.__name__
                raise TypeError(
                    f"Cannot subclass an instance of NewType. Perhaps you were looking for: "
                    f"`{subclass_name} = NewType({subclass_name!r}, {superclass_name})`"
                )

        return (Dummy,)

    def __repr__(self):
        return f'{self.__module__}.{self.__qualname__}'

    def __reduce__(self):
        return self.__qualname__

    def __or__(self, other):
        return _typing.Union[self, other]

    def __ror__(self, other):
        return _typing.Union[other, self]


T = TypeVar("T")
KT = TypeVar("KT")
VT = TypeVar("VT")
AnyStr = TypeVar("AnyStr", bytes, str)
Text = str


# Protocol introspection from CPython 3.14.5 Lib/typing.py. See stdlib/docstrings/LICENSE.
def is_protocol(tp: type, /) -> _builtins.bool:
    """Return True if the given type is a Protocol.

    Example::

        >>> from typing import Protocol, is_protocol
        >>> class P(Protocol):
        ...     def a(self) -> str: ...
        ...     b: int
        >>> is_protocol(P)
        True
        >>> is_protocol(int)
        False
    """
    return (
        isinstance(tp, type)
        and getattr(tp, '_is_protocol', False)
        and tp != _typing.Protocol
    )


_TYPING_INTERNALS = frozenset({
    '__parameters__', '__orig_bases__',  '__orig_class__',
    '_is_protocol', '_is_runtime_protocol', '__protocol_attrs__',
    '__non_callable_proto_members__', '__type_params__',
})

_SPECIAL_NAMES = frozenset({
    '__abstractmethods__', '__annotations__', '__dict__', '__doc__',
    '__init__', '__module__', '__new__', '__slots__',
    '__subclasshook__', '__weakref__', '__class_getitem__',
    '__match_args__', '__static_attributes__', '__firstlineno__',
    '__annotate__', '__annotate_func__', '__annotations_cache__',
})

# These special attributes will be not collected as protocol members.
EXCLUDED_ATTRIBUTES = _TYPING_INTERNALS | _SPECIAL_NAMES | {'_MutableMapping__marker'}


_protocol_annotation_names = None


def _freeze_protocol_members(cls, declared_annotations):
    global _protocol_annotation_names
    provider = cls.__dict__.get('__annotate_func__')
    if declared_annotations and provider is not None:
        # Key the fallback by the original generated provider. A replacement
        # must supply its own annotations and the cache must not retain classes.
        if _protocol_annotation_names is None:
            from weakref import WeakKeyDictionary
            _protocol_annotation_names = WeakKeyDictionary()
        _protocol_annotation_names[provider] = tuple(declared_annotations)
    cls.__protocol_attrs__ = _get_protocol_attrs(cls)


def _get_protocol_attrs(cls):
    """Collect protocol members from a protocol class objects.

    This includes names actually defined in the class dictionary, as well
    as names that appear in annotations. Special names (above) are skipped.
    """
    attrs = set()
    for base in cls.__mro__[:-1]:  # without object
        if base.__name__ in {'Protocol', 'Generic'}:
            continue
        annotations = _protocol_annotations(base)
        for attr in (*base.__dict__, *annotations):
            if not attr.startswith('_abc_') and attr not in EXCLUDED_ATTRIBUTES:
                attrs.add(attr)
    return attrs


def get_protocol_members(tp: type, /) -> frozenset[str]:
    """Return the set of members defined in a Protocol.

    Example::

        >>> from typing import Protocol, get_protocol_members
        >>> class P(Protocol):
        ...     def a(self) -> str: ...
        ...     b: int
        >>> get_protocol_members(P) == frozenset({'a', 'b'})
        True

    Raise a TypeError for arguments that are not Protocols.
    """
    if not is_protocol(tp):
        raise TypeError(f'{tp!r} is not a Protocol')
    return frozenset(tp.__protocol_attrs__)


def _protocol_annotations(base):
    try:
        return base.__annotations__
    except Exception:
        # The class builder records executed annotation declarations even
        # when a forward reference prevents evaluating their values.
        provider = getattr(base, '__annotate__', None)
        if provider is None:
            return ()
        else:
            try:
                return provider(3)  # annotationlib.Format.FORWARDREF
            except NotImplementedError:
                if _protocol_annotation_names is None or provider not in _protocol_annotation_names:
                    raise
                return _protocol_annotation_names[provider]


@classmethod
def _proto_hook(cls, other):
    if not cls.__dict__.get('_is_protocol', False):
        return NotImplemented

    missing = object()
    for attr in cls.__protocol_attrs__:
        for base in _typing._protocol_mro(other):
            member = _typing._protocol_member(base, attr, missing)
            if member is not missing:
                if member is None:
                    return NotImplemented
                break

            # ...or in annotations, if it is a sub-protocol.
            if issubclass(other, Generic) and getattr(other, "_is_protocol", False):
                if attr in _protocol_annotations(base):
                    break
        else:
            return NotImplemented
    return True


def runtime_checkable(cls):
    """Mark a protocol class as a runtime protocol.

    Such protocol can be used with isinstance() and issubclass().
    Raise TypeError if applied to a non-protocol class.
    This allows a simple-minded structural check very similar to
    one trick ponies in collections.abc such as Iterable.

    For example::

        @runtime_checkable
        class Closable(Protocol):
            def close(self): ...

        assert isinstance(open('/some/file'), Closable)

    Warning: this will check only the presence of the required methods,
    not their type signatures!
    """
    if not issubclass(cls, Generic) or not getattr(cls, '_is_protocol', False):
        raise TypeError('@runtime_checkable can be only applied to protocol classes,'
                        ' got %r' % cls)
    cls._is_runtime_protocol = True
    # PEP 544 prohibits using issubclass()
    # with protocols that have non-method members.
    # See gh-113320 for why we compute this attribute here,
    # rather than in `_ProtocolMeta.__init__`
    cls.__non_callable_proto_members__ = set()
    for attr in cls.__protocol_attrs__:
        try:
            is_callable = callable(getattr(cls, attr, None))
        except Exception as e:
            raise TypeError(
                f"Failed to determine whether protocol member {attr!r} "
                "is a method member"
            ) from e
        else:
            if not is_callable:
                cls.__non_callable_proto_members__.add(attr)
    return cls


def _check_protocol(cls, subclass):
    if not getattr(cls, '_is_runtime_protocol', False):
        raise TypeError("Instance and class checks can only be used with @runtime_checkable protocols")
    if (subclass and cls.__non_callable_proto_members__
            and cls.__dict__.get('__subclasshook__') is _proto_hook):
        names = sorted(cls.__non_callable_proto_members__)
        raise TypeError("Protocols with non-method members don't support issubclass()."
                        f" Non-method members: {str(names)[1:-1]}.")


def _protocol_instance(cls, instance):
    from inspect import getattr_static
    for attr in cls.__protocol_attrs__:
        try:
            val = getattr_static(instance, attr)
        except AttributeError:
            return False
        if val is None and attr not in cls.__non_callable_proto_members__:
            return False
    return True


# Runtime protocols from CPython 3.14.5 Lib/typing.py.
@runtime_checkable
class SupportsInt(Protocol):
    """An ABC with one abstract method __int__."""

    __slots__ = ()

    @abstractmethod
    def __int__(self) -> int:
        pass


@runtime_checkable
class SupportsFloat(Protocol):
    """An ABC with one abstract method __float__."""

    __slots__ = ()

    @abstractmethod
    def __float__(self) -> float:
        pass


@runtime_checkable
class SupportsComplex(Protocol):
    """An ABC with one abstract method __complex__."""

    __slots__ = ()

    @abstractmethod
    def __complex__(self) -> complex:
        pass


@runtime_checkable
class SupportsBytes(Protocol):
    """An ABC with one abstract method __bytes__."""

    __slots__ = ()

    @abstractmethod
    def __bytes__(self) -> bytes:
        pass


@runtime_checkable
class SupportsIndex(Protocol):
    """An ABC with one abstract method __index__."""

    __slots__ = ()

    @abstractmethod
    def __index__(self) -> int:
        pass


@runtime_checkable
class SupportsAbs[T](Protocol):
    """An ABC with one abstract method __abs__ that is covariant in its return type."""

    __slots__ = ()

    @abstractmethod
    def __abs__(self) -> T:
        pass


@runtime_checkable
class SupportsRound[T](Protocol):
    """An ABC with one abstract method __round__ that is covariant in its return type."""

    __slots__ = ()

    @abstractmethod
    def __round__(self, ndigits: int = 0) -> T:
        pass


class IO(Generic[AnyStr]):
    __slots__ = ()


class BinaryIO(IO[bytes]):
    __slots__ = ()


class TextIO(IO[str]):
    __slots__ = ()


# TypedDict creation and nominal recognition from CPython 3.14.5 Lib/typing.py.
class _TypedDictMeta(type):
    __call__ = dict

    def __subclasscheck__(cls, other):
        raise TypeError("TypedDict does not support instance and class checks")

    __instancecheck__ = __subclasscheck__


def _typeddict_field(value, module, owner=None):
    if value is None:
        return type(None)
    if isinstance(value, str):
        return _make_forward_ref(value, module, owner)
    message = "TypedDict('Name', {f0: t0, f1: t1, ...}); each t must be a type"
    return _type_check(value, message)


def _typeddict_qualifiers(tp):
    qualifiers = set()
    annotated = getattr(_typing, "Annotated", None)
    while True:
        origin = _typing.get_origin(tp)
        if origin is not None and origin is annotated:
            args = _typing.get_args(tp)
            if not args:
                break
            tp = args[0]
        elif origin is Required or origin is NotRequired or origin is ReadOnly:
            qualifiers.add(origin)
            tp, = _typing.get_args(tp)
        else:
            break
    return qualifiers


def _annotation_type_repr(value):
    # Equivalent to annotationlib.type_repr for the supported runtime values.
    from types import BuiltinFunctionType, FunctionType

    if value is None:
        return "None"
    if value is type(None):
        return "NoneType"
    if isinstance(value, (type, FunctionType, BuiltinFunctionType)):
        return (value.__qualname__ if value.__module__ == "builtins" else
                f"{value.__module__}.{value.__qualname__}")
    if value is ...:
        return "..."
    from string.templatelib import Template
    if isinstance(value, Template):
        raise NotImplementedError("template annotation strings require AST unparsing")
    return repr(value)


def _annotation_strings(annotations):
    return {name: value if isinstance(value, str) else _annotation_type_repr(value)
            for name, value in annotations.items()}


class _TypedDictSymbol:
    """An operand in a synthetic annotation namespace (see annotationlib._Stringifier)."""

    __slots__ = ForwardRef.__slots__

    def __expression__(self):
        node = self.__ast_node__
        return ((node,), 20) if type(node) is str else node

    def __repr__(self):
        tokens, _ = self.__expression__()
        return "".join(t if type(t) is str else t[0] for t in tokens)

    def __hash__(self):
        return id(self)

    def __operand__(self, value, precedence=0):
        if type(value) is _TypedDictSymbol:
            tokens, priority = value.__expression__()
            return ("(", *tokens, ")") if priority < precedence else tokens
        if type(value) in (list, tuple, set, dict):
            if type(value) is set and not value:
                return ("{*()}",)
            opening, closing = ("[", "]") if type(value) is list else (
                ("(", ")") if type(value) is tuple else ("{", "}"))
            tokens = [opening]
            for i, item in enumerate(value):
                if i:
                    tokens.append(", ")
                tokens.extend(self.__operand__(item))
                if type(value) is dict:
                    tokens.append(": ")
                    tokens.extend(self.__operand__(value[item]))
            if type(value) is tuple and len(value) == 1:
                tokens.append(",")
            tokens.append(closing)
            return tuple(tokens)
        if self.__stringifier_dict__.symbolic and value is ...:
            return ("...",)
        if self.__stringifier_dict__.symbolic or value is None or type(value) in (str, int, float, bool, complex):
            return (repr(value),)
        # Real operands may have no name in the eventual evaluation namespace.
        names = self.__stringifier_dict__
        key = f"__annotationlib_name_{names.next_id}__"
        names.next_id += 1
        return ((key, value),)

    def __derived__(self, tokens, precedence=20):
        return self.__stringifier_dict__.symbol(tokens, None, precedence)

    def __getattr__(self, name):
        return self.__derived__((*self.__operand__(self, 20), ".", name))

    def __getitem__(self, index):
        tokens = [*self.__operand__(self, 20), "["]
        items = index if isinstance(index, tuple) and index else (index,)
        for i, item in enumerate(items):
            if i:
                tokens.append(", ")
            if isinstance(item, slice):
                if item.start is not None:
                    tokens.extend(self.__operand__(item.start))
                tokens.append(":")
                if item.stop is not None:
                    tokens.extend(self.__operand__(item.stop))
                if item.step is not None:
                    tokens.append(":")
                    tokens.extend(self.__operand__(item.step))
            else:
                tokens.extend(self.__operand__(item))
        if isinstance(index, tuple) and len(index) == 1:
            tokens.append(",")
        tokens.append("]")
        return self.__derived__(tuple(tokens))

    def __call__(self, *args, **kwargs):
        tokens = [*self.__operand__(self, 20), "("]
        for arg in args:
            tokens.extend(self.__operand__(arg))
            tokens.append(", ")
        for key, value in kwargs.items():
            tokens.extend((key, "="))
            tokens.extend(self.__operand__(value))
            tokens.append(", ")
        if args or kwargs:
            tokens.pop()
        tokens.append(")")
        return self.__derived__(tuple(tokens))

    def __contains__(self, item):
        if self.__ast_node__ == "__conditional_annotations__":
            return True
        raise NotImplementedError("symbolic annotation membership")

    def __format__(self, spec):
        raise TypeError("Cannot stringify annotation containing string formatting")

    def _binary(op, precedence, *, reverse=False, comparison=False):
        def binary(self, other):
            left, right = (other, self) if reverse else (self, other)
            return self.__derived__(
                (*self.__operand__(left, precedence + comparison), f" {op} ",
                 *self.__operand__(right, precedence + 1)), precedence)
        return binary

    __add__ = _binary("+", 10)
    __radd__ = _binary("+", 10, reverse=True)
    __or__ = _binary("|", 6)
    __ror__ = _binary("|", 6, reverse=True)
    __eq__ = _binary("==", 5, comparison=True)
    __ne__ = _binary("!=", 5, comparison=True)
    __lt__ = _binary("<", 5, comparison=True)
    __le__ = _binary("<=", 5, comparison=True)
    __gt__ = _binary(">", 5, comparison=True)
    __ge__ = _binary(">=", 5, comparison=True)
    del _binary


class _TypedDictNames(dict):
    """Resolve missing names without modifying the provider's globals or cells."""

    def symbol(self, tokens, cells, precedence=20):
        value = _TypedDictSymbol()
        value.__arg__ = None
        value.__ast_node__ = tokens[0] if len(tokens) == 1 else (tokens, precedence)
        value.__code__ = None
        value.__stringifier_dict__ = self
        value.__globals__ = self.source_globals
        value.__cell__ = cells
        value.__forward_module__ = None
        value.__forward_is_class__ = self.is_class
        value.__forward_is_argument__ = False
        value.__owner__ = self.owner
        value.__extra_names__ = dict(t for t in tokens if type(t) is tuple) or None
        value.__resolved_str_cache__ = None
        self.symbols.append(value)
        return value

    def __missing__(self, name):
        return self.symbol((name,), None)

    def finish(self):
        # Convert the actual operands as in CPython. They can be held inside a
        # typing alias, an arbitrary container, or a callback's saved state.
        for value in self.symbols:
            tokens, _ = value.__expression__()
            value.__arg__ = repr(value)
            value.__ast_node__ = tokens if value.__extra_names__ else None
            value.__stringifier_dict__ = None
            if value.__cell__ is None:
                value.__cell__ = self.cells
            _typing._finish_field_ref(value)
        self.symbols.clear()


def _typeddict_names(source_globals, cells, owner, known, is_class=None):
    names = _TypedDictNames()
    names.source_globals = source_globals
    names.cells = cells
    names.owner = owner
    names.is_class = isinstance(owner, type) if is_class is None else is_class
    names.symbolic = known is None
    names.symbols = []
    names.next_id = 1
    if known is not None:
        names.update(known)
    return names


def _typeddict_cell(value):
    return (lambda: value).__closure__[0]


def _typeddict_replay(annotate, owner, allow_evaluation):
    from types import FunctionType
    cells = (dict(zip(annotate.__code__.co_freevars, annotate.__closure__))
             if annotate.__closure__ else None)
    known = {**annotate.__builtins__, **annotate.__globals__} if allow_evaluation else None
    names = _typeddict_names(annotate.__globals__, cells, owner, known)
    closure = None
    if cells:
        new_cells = []
        for name, cell in cells.items():
            if allow_evaluation:
                try:
                    cell.cell_contents
                except ValueError:
                    value = names.symbol((name,), cell)
                else:
                    new_cells.append(cell)
                    continue
            elif name == "__classdict__":
                # A miss here must fall through to the free variable or global.
                value = {}
            else:
                value = names.symbol((name,), cell)
            new_cells.append(_typeddict_cell(value))
        closure = tuple(new_cells)
    func = FunctionType(annotate.__code__, names, argdefs=annotate.__defaults__,
                        closure=closure, kwdefaults=annotate.__kwdefaults__)
    return func, names


def _call_annotate(annotate, format, owner=None):
    from annotationlib import Format
    if format == Format.VALUE_WITH_FAKE_GLOBALS:
        raise ValueError("The VALUE_WITH_FAKE_GLOBALS format is for internal use only")
    try:
        return annotate(format)
    except NotImplementedError:
        pass

    if format == Format.VALUE:
        raise RuntimeError("annotate function does not support VALUE format")
    if format not in (Format.FORWARDREF, Format.STRING):
        raise ValueError(f"Invalid format: {format!r}")

    if format == Format.FORWARDREF:
        probe, names = _typeddict_replay(annotate, owner, True)
    else:
        probe = annotate
    try:
        result = probe(Format.VALUE_WITH_FAKE_GLOBALS)
    except NotImplementedError:
        values = annotate(Format.VALUE)
        return _annotation_strings(values) if format == Format.STRING else values
    except Exception:
        pass
    else:
        if format == Format.FORWARDREF:
            names.finish()
            return result

    # Known names can themselves raise on evaluation. Re-run the provider with
    # symbolic names, keeping its constants, control flow and exception behavior.
    symbolic, names = _typeddict_replay(annotate, owner, False)
    result = symbolic(Format.VALUE_WITH_FAKE_GLOBALS)
    if format == Format.STRING:
        return {key: value if isinstance(value, str) else
                "..." if value is ... else repr(value) for key, value in result.items()}
    names.finish()
    return {key: value.evaluate(format=Format.FORWARDREF)
            if isinstance(value, ForwardRef) else value for key, value in result.items()}


_sentinel = object()


def _make_eager_annotate(types=_sentinel):
    if types is _sentinel:
        return lambda format: {}
    checked = {key: _type_check(value, f"field {key} annotation must be a type")
               for key, value in types.items()}

    def annotate(format):
        if format in (1, 3):
            return checked
        if format == 4:
            return _annotation_strings(types)
        raise NotImplementedError(format)

    return annotate


# Functional NamedTuple from CPython 3.14.5 Lib/typing.py.
def NamedTuple(typename, fields=_sentinel, /, **kwargs):
    """Typed version of namedtuple.

    Usage::

        class Employee(NamedTuple):
            name: str
            id: int

    This is equivalent to::

        Employee = collections.namedtuple('Employee', ['name', 'id'])

    The resulting class has an extra __annotations__ attribute, giving a
    dict that maps field names to types.  (The field names are also in
    the _fields attribute, which is part of the namedtuple API.)
    An alternative equivalent functional syntax is also accepted::

        Employee = NamedTuple('Employee', [('name', str), ('id', int)])
    """
    from collections import namedtuple

    if fields is _sentinel:
        if kwargs:
            deprecated_thing = "Creating NamedTuple classes using keyword arguments"
            deprecation_msg = (
                "{name} is deprecated and will be disallowed in Python {remove}. "
                "Use the class-based or functional syntax instead."
            )
        else:
            deprecated_thing = "Failing to pass a value for the 'fields' parameter"
            example = f"`{typename} = NamedTuple({typename!r}, [])`"
            deprecation_msg = (
                "{name} is deprecated and will be disallowed in Python {remove}. "
                "To create a NamedTuple class with 0 fields "
                "using the functional syntax, "
                "pass an empty list, e.g. "
            ) + example + "."
    elif fields is None:
        if kwargs:
            raise TypeError(
                "Cannot pass `None` as the 'fields' parameter "
                "and also specify fields using keyword arguments"
            )
        else:
            deprecated_thing = "Passing `None` as the 'fields' parameter"
            example = f"`{typename} = NamedTuple({typename!r}, [])`"
            deprecation_msg = (
                "{name} is deprecated and will be disallowed in Python {remove}. "
                "To create a NamedTuple class with 0 fields "
                "using the functional syntax, "
                "pass an empty list, e.g. "
            ) + example + "."
    elif kwargs:
        raise TypeError("Either list of fields or keywords"
                        " can be provided to NamedTuple, not both")
    if fields is _sentinel or fields is None:
        import warnings
        warnings._deprecated(deprecated_thing, message=deprecation_msg, remove=(3, 15))
        fields = kwargs.items()
    types = {n: _type_check(t, f"field {n} annotation must be a type")
             for n, t in fields}
    field_names = [n for n, _ in fields]

    annotate = _make_eager_annotate(types)
    nt = namedtuple(typename, field_names, defaults=(), module=_caller())
    nt.__annotate__ = nt.__new__.__annotate__ = annotate
    nt.__orig_bases__ = (NamedTuple,)
    return nt


def _namedtuple_mro_entries(bases):
    assert NamedTuple in bases
    return (_typing._NamedTuple,)


NamedTuple.__mro_entries__ = _namedtuple_mro_entries


def _namedtuple_set_names(cls, namespace):
    typename = cls.__name__
    for name, value in namespace.items():
        if name in ('__new__', '__init__', '__slots__', '__getnewargs__',
                    '_fields', '_field_defaults', '_make', '_replace', '_asdict', '_source'):
            raise AttributeError('Cannot overwrite NamedTuple attribute ' + name)
        if name in ('__module__', '__name__', '__annotations__', '__annotate__',
                    '__annotate_func__', '__annotations_cache__'):
            continue
        if name not in cls._fields:
            setattr(cls, name, value)
        try:
            set_name = type(value).__set_name__
        except AttributeError:
            continue
        try:
            set_name(value, cls, name)
        except BaseException as exc:
            exc.add_note(
                f"Error calling __set_name__ on {type(value).__name__!r} "
                f"instance {name!r} in {typename!r}"
            )
            raise


def _namedtuple_generic_getitem(cls, args):
    return _specialize_generic_alias(_typing._GenericAlias(cls, cls.__parameters__), args)


def _namedtuple_annotate(original):
    # Check field types at declaration time, even if a name is still undefined.
    def annotate(format):
        values = _call_annotate(original, format)
        if format == 4:
            return values
        return {name: _type_check(value, f"field {name} annotation must be a type")
                for name, value in values.items()}

    return annotate


def _configure_typeddict(cls, fields, total, own_annotate=None, bases=()):
    if not hasattr(cls, "__orig_bases__"):
        cls.__orig_bases__ = bases

    if own_annotate is not None:
        fields = _call_annotate(own_annotate, 3, cls)
    annotations = {key: _typeddict_field(tp, cls.__module__, cls)
                   for key, tp in fields.items()}

    def __annotate__(format):
        result = {}
        for base in bases:
            if base is Generic:
                continue
            annotate = base.__annotate__
            if annotate is not None:
                result.update(_call_annotate(annotate, format, base))
        if own_annotate is not None:
            own = _call_annotate(own_annotate, format, cls)
            if format != 4:
                own = {key: _typeddict_field(tp, cls.__module__) for key, tp in own.items()}
        elif format == 4:
            own = _annotation_strings(fields)
        elif format in (1, 3):
            own = annotations
        else:
            raise NotImplementedError(format)
        result.update(own)
        return result

    cls.__annotate__ = __annotate__
    required = set()
    optional = set()
    readonly = set()
    mutable = set()
    for base in bases:
        namespace = base.__dict__
        base_required = namespace.get("__required_keys__", set())
        required |= base_required
        optional -= base_required
        base_optional = namespace.get("__optional_keys__", set())
        required -= base_optional
        optional |= base_optional
        readonly.update(namespace.get("__readonly_keys__", ()))
        mutable.update(namespace.get("__mutable_keys__", ()))
    for key, tp in annotations.items():
        qualifiers = _typeddict_qualifiers(tp)
        if Required in qualifiers or (NotRequired not in qualifiers and total):
            required.add(key)
            optional.discard(key)
        else:
            optional.add(key)
            required.discard(key)
        if ReadOnly in qualifiers:
            if key in mutable:
                raise TypeError(f"Cannot override mutable key {key!r} with read-only key")
            readonly.add(key)
        else:
            mutable.add(key)
            readonly.discard(key)
    cls.__required_keys__ = frozenset(required)
    cls.__optional_keys__ = frozenset(optional)
    cls.__readonly_keys__ = frozenset(readonly)
    cls.__mutable_keys__ = frozenset(mutable)
    cls.__total__ = total


def TypedDict(typename, fields=_sentinel, /, *, total=True):
    """A simple typed namespace. At runtime it is equivalent to a plain dict.

    TypedDict creates a dictionary type such that a type checker will expect all
    instances to have a certain set of keys, where each key is
    associated with a value of a consistent type. This expectation
    is not checked at runtime.

    Usage::

        >>> class Point2D(TypedDict):
        ...     x: int
        ...     y: int
        ...     label: str
        ...
        >>> a: Point2D = {'x': 1, 'y': 2, 'label': 'good'}  # OK
        >>> b: Point2D = {'z': 3, 'label': 'bad'}           # Fails type check
        >>> Point2D(x=1, y=2, label='first') == dict(x=1, y=2, label='first')
        True

    The type info can be accessed via the Point2D.__annotations__ dict, and
    the Point2D.__required_keys__ and Point2D.__optional_keys__ frozensets.
    TypedDict supports an additional equivalent form::

        Point2D = TypedDict('Point2D', {'x': int, 'y': int, 'label': str})

    By default, all keys must be present in a TypedDict. It is possible
    to override this by specifying totality::

        class Point2D(TypedDict, total=False):
            x: int
            y: int

    This means that a Point2D TypedDict can have any of the keys omitted. A type
    checker is only expected to support a literal False or True as the value of
    the total argument. True is the default, and makes all items defined in the
    class body be required.

    The Required and NotRequired special forms can also be used to mark
    individual keys as being required or not required::

        class Point2D(TypedDict):
            x: int               # the "x" key must always be present (Required is the default)
            y: NotRequired[int]  # the "y" key can be omitted

    See PEP 655 for more details on Required and NotRequired.

    The ReadOnly special form can be used
    to mark individual keys as immutable for type checkers::

        class DatabaseUser(TypedDict):
            id: ReadOnly[int]  # the "id" key must not be modified
            username: str      # the "username" key can be changed

    """
    if fields is _sentinel or fields is None:
        import warnings

        if fields is _sentinel:
            deprecated_thing = "Failing to pass a value for the 'fields' parameter"
        else:
            deprecated_thing = "Passing `None` as the 'fields' parameter"

        example = f"`{typename} = TypedDict({typename!r}, {{{{}}}})`"
        deprecation_msg = (
            "{name} is deprecated and will be disallowed in Python {remove}. "
            "To create a TypedDict class with 0 fields "
            "using the functional syntax, "
            "pass an empty dictionary, e.g. "
        ) + example + "."
        warnings._deprecated(deprecated_thing, message=deprecation_msg, remove=(3, 15))
        fields = {}

    fields = dict(fields)
    td = _typing._new_typeddict(typename, False)
    td.__orig_bases__ = (TypedDict,)
    _configure_typeddict(td, fields, total)
    return td


_TypedDict = _typing._new_typeddict("TypedDict", True)
TypedDict.__mro_entries__ = lambda bases: (_TypedDict,)


def get_origin(tp):
    """Get the unsubscripted version of a type.

    This supports generic types, Callable, Tuple, Union, Literal, Final, ClassVar,
    Annotated, and others. Return None for unsupported types.

    Examples::

        >>> P = ParamSpec('P')
        >>> assert get_origin(Literal[42]) is Literal
        >>> assert get_origin(int) is None
        >>> assert get_origin(ClassVar[int]) is ClassVar
        >>> assert get_origin(Generic) is Generic
        >>> assert get_origin(Generic[T]) is Generic
        >>> assert get_origin(Union[T, int]) is Union
        >>> assert get_origin(List[Tuple[T, T]][int]) is list
        >>> assert get_origin(P.args) is P
    """
    return _typing_runtime._get_origin(tp)


def get_args(tp):
    """Get type arguments with all substitutions performed.

    For unions, basic simplifications used by Union constructor are performed.

    Examples::

        >>> T = TypeVar('T')
        >>> assert get_args(Dict[str, int]) == (str, int)
        >>> assert get_args(int) == ()
        >>> assert get_args(Union[int, Union[T, int], str][int]) == (int, str)
        >>> assert get_args(Union[int, Tuple[T, int]][str]) == (int, Tuple[str, int])
        >>> assert get_args(Callable[[], T][int]) == ([], int)
    """
    return _typing_runtime._get_args(tp)


def is_typeddict(tp):
    """Check if an annotation is a TypedDict class.

    For example::

        >>> from typing import TypedDict
        >>> class Film(TypedDict):
        ...     title: str
        ...     year: int
        ...
        >>> is_typeddict(Film)
        True
        >>> is_typeddict(dict)
        False
    """
    return isinstance(tp, _TypedDictMeta)


# Runtime assertions from CPython 3.14.5 Lib/typing.py. See stdlib/docstrings/LICENSE.
def assert_type(val, typ, /):
    """Ask a static type checker to confirm that the value is of the given type.

    At runtime this does nothing: it returns the first argument unchanged with no
    checks or side effects, no matter the actual type of the argument.

    When a static type checker encounters a call to assert_type(), it
    emits an error if the value is not of the specified type::

        def greet(name: str) -> None:
            assert_type(name, str)  # OK
            assert_type(name, int)  # type checker error
    """
    return val


def reveal_type[T](obj: T, /) -> T:
    """Ask a static type checker to reveal the inferred type of an expression.

    When a static type checker encounters a call to ``reveal_type()``,
    it will emit the inferred type of the argument::

        x: int = 1
        reveal_type(x)

    Running a static type checker (e.g., mypy) on this example
    will produce output similar to 'Revealed type is "builtins.int"'.

    At runtime, the function prints the runtime type of the
    argument and returns the argument unchanged.
    """
    print(f"Runtime type is {type(obj).__name__!r}", file=_sys.stderr)
    return obj


_ASSERT_NEVER_REPR_MAX_LENGTH = 100


def assert_never(arg, /):
    """Statically assert that a line of code is unreachable."""
    value = repr(arg)
    if len(value) > _ASSERT_NEVER_REPR_MAX_LENGTH:
        value = value[:_ASSERT_NEVER_REPR_MAX_LENGTH] + "..."
    raise AssertionError(f"Expected code to be unreachable, but got: {value}")


# Runtime decorator from CPython 3.14.5 Lib/typing.py. See stdlib/docstrings/LICENSE.
def final(f):
    """Decorator to indicate final methods and final classes.

    Use this decorator to indicate to type checkers that the decorated
    method cannot be overridden, and decorated class cannot be subclassed.

    For example::

        class Base:
            @final
            def done(self) -> None:
                ...
        class Sub(Base):
            def done(self) -> None:  # Error reported by type checker
                ...

        @final
        class Leaf:
            ...
        class Other(Leaf):  # Error reported by type checker
            ...

    There is no runtime checking of these properties. The decorator
    attempts to set the ``__final__`` attribute to ``True`` on the decorated
    object to allow runtime introspection.
    """
    try:
        f.__final__ = True
    except (AttributeError, TypeError):
        # Skip the attribute silently if it is not writable.
        # AttributeError happens if the object has __slots__ or a
        # read-only property, TypeError if it's a builtin class.
        pass
    return f


# Based on CPython 3.14.5 Lib/typing.py. See stdlib/docstrings/LICENSE.
def override(method, /):
    """Indicate that a method is intended to override a method in a base class.

    Usage::

        class Base:
            def method(self) -> None:
                pass

        class Child(Base):
            @override
            def method(self) -> None:
                super().method()

    When this decorator is applied to a method, the type checker will
    validate that it overrides a method or attribute with the same name on a
    base class.  This helps prevent bugs that may occur when a base class is
    changed without an equivalent change to a child class.

    There is no runtime checking of this property. The decorator attempts to
    set the ``__override__`` attribute to ``True`` on the decorated object to
    allow runtime introspection.

    See PEP 698 for details.
    """
    try:
        method.__override__ = True
    except (AttributeError, TypeError):
        # Skip the attribute silently if it is not writable.
        # AttributeError happens if the object has __slots__ or a
        # read-only property, TypeError if it's a builtin class.
        pass
    return method


def no_type_check(arg):
    """Decorator to indicate that annotations are not type hints.

    The argument must be a class or function; if it is a class, it
    applies recursively to all methods and classes defined in that class
    (but not to methods defined in its superclasses or subclasses).

    This mutates the function(s) or class(es) in place.
    """
    if isinstance(arg, type):
        from types import FunctionType, MethodType

        if type(arg) is type:
            # Only Python class dictionaries can contain definitions to mark.
            # Native descriptor names advertised by dir() need not be materialized.
            names = set()
            for base in getattr(arg, '__mro__', (arg,)):
                for key, value in getattr(base, '__dict__', {}).items():
                    if key == '__annotations__' and type(value) is dict:
                        # An annotations dict cannot be a declared function or class.
                        # Looking it up can force unrelated deferred annotations.
                        continue
                    names.add(key)
            names = sorted(names)
        else:
            names = dir(arg)
        for key in names:
            obj = getattr(arg, key)
            if (
                not hasattr(obj, '__qualname__')
                or obj.__qualname__ != f'{arg.__qualname__}.{obj.__name__}'
                or getattr(obj, '__module__', None) != arg.__module__
            ):
                # We only modify objects that are defined in this type directly.
                # If classes / methods are nested in multiple layers,
                # we will modify them when processing their direct holders.
                continue
            # Instance, class, and static methods:
            if isinstance(obj, FunctionType):
                obj.__no_type_check__ = True
            if isinstance(obj, MethodType):
                obj.__func__.__no_type_check__ = True
            # Nested types:
            if isinstance(obj, type):
                no_type_check(obj)
    try:
        arg.__no_type_check__ = True
    except TypeError:  # built-in classes
        pass
    return arg


def no_type_check_decorator(decorator):
    """Decorator to give another decorator the @no_type_check effect.

    This wraps the decorator with something that wraps the decorated
    function in @no_type_check.
    """
    import warnings
    warnings._deprecated("typing.no_type_check_decorator", remove=(3, 15))
    @functools.wraps(decorator)
    def wrapped_decorator(*args, **kwds):
        func = decorator(*args, **kwds)
        func = no_type_check(func)
        return func

    return wrapped_decorator


# Optional and the special form protocol from CPython 3.14.5 Lib/typing.py.
# See stdlib/docstrings/LICENSE. Union and the type parameters are native exports.
class _Final:
    __module__ = __name__
    __slots__ = ("__weakref__",)

    def __init_subclass__(cls, /, *args, **kwds):
        if '_root' not in kwds:
            raise TypeError("Cannot subclass special typing classes")


class _NotIterable:
    __module__ = __name__
    __slots__ = ()
    __iter__ = None


class _SpecialForm(_Final, _NotIterable, _root=True):
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


def _type_convert(arg, module=None, *, allow_special_forms=False, owner=None):
    if arg is None:
        return type(None)
    if isinstance(arg, str):
        return _make_forward_ref(arg, module, owner, is_class=allow_special_forms)
    return arg


def _type_check(arg, message, is_argument=True, module=None, *, allow_special_forms=False, owner=None):
    arg = _type_convert(arg, module, allow_special_forms=allow_special_forms, owner=owner)
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
        or (not allow_special_forms and (origin is ClassVar or (is_argument and origin is Final)))
    ):
        raise TypeError(f"{repr(arg)} is not valid as type argument")
    if type(arg) is tuple:
        raise TypeError(f"{message} Got {repr(arg)[:100]}.")
    return arg


def _typeddict_qualifier_alias(form, parameters):
    item = _type_check(parameters, f"{form._name} accepts only a single type.")
    if isinstance(item, str):
        item = _make_forward_ref(item)
    return _typing._GenericAlias(form, (type(None) if item is None else item,))


@_SpecialForm
def Required(self, parameters):
    """Special typing construct to mark a TypedDict key as required.

    This is mainly useful for total=False TypedDicts.

    For example::

        class Movie(TypedDict, total=False):
            title: Required[str]
            year: int

        m = Movie(
            title='The Matrix',  # typechecker error if key is omitted
            year=1999,
        )

    There is no runtime checking that a required key is actually provided
    when instantiating a related TypedDict.
    """
    return _typeddict_qualifier_alias(self, parameters)


@_SpecialForm
def NotRequired(self, parameters):
    """Special typing construct to mark a TypedDict key as potentially missing.

    For example::

        class Movie(TypedDict):
            title: str
            year: NotRequired[int]

        m = Movie(
            title='The Matrix',  # typechecker error if key is omitted
            year=1999,
        )
    """
    return _typeddict_qualifier_alias(self, parameters)


@_SpecialForm
def ReadOnly(self, parameters):
    """A special typing construct to mark an item of a TypedDict as read-only.

    For example::

        class Movie(TypedDict):
            title: ReadOnly[str]
            year: int

        def mutate_movie(m: Movie) -> None:
            m["year"] = 1992  # allowed
            m["title"] = "The Matrix"  # typechecker error

    There is no runtime checking for this property.
    """
    return _typeddict_qualifier_alias(self, parameters)


@_SpecialForm
def TypeGuard(self, parameters):
    """Special typing construct for marking user-defined type predicate functions.

    ``TypeGuard`` can be used to annotate the return type of a user-defined
    type predicate function.  ``TypeGuard`` only accepts a single type argument.
    At runtime, functions marked this way should return a boolean.

    ``TypeGuard`` aims to benefit *type narrowing* -- a technique used by static
    type checkers to determine a more precise type of an expression within a
    program's code flow.  Usually type narrowing is done by analyzing
    conditional code flow and applying the narrowing to a block of code.  The
    conditional expression here is sometimes referred to as a "type predicate".

    Sometimes it would be convenient to use a user-defined boolean function
    as a type predicate.  Such a function should use ``TypeGuard[...]`` or
    ``TypeIs[...]`` as its return type to alert static type checkers to
    this intention. ``TypeGuard`` should be used over ``TypeIs`` when narrowing
    from an incompatible type (e.g., ``list[object]`` to ``list[int]``) or when
    the function does not return ``True`` for all instances of the narrowed type.

    Using  ``-> TypeGuard[NarrowedType]`` tells the static type checker that
    for a given function:

    1. The return value is a boolean.
    2. If the return value is ``True``, the type of its argument
       is ``NarrowedType``.

    For example::

         def is_str_list(val: list[object]) -> TypeGuard[list[str]]:
             '''Determines whether all objects in the list are strings'''
             return all(isinstance(x, str) for x in val)

         def func1(val: list[object]):
             if is_str_list(val):
                 # Type of ``val`` is narrowed to ``list[str]``.
                 print(" ".join(val))
             else:
                 # Type of ``val`` remains as ``list[object]``.
                 print("Not a list of strings!")

    Strict type narrowing is not enforced -- ``TypeB`` need not be a narrower
    form of ``TypeA`` (it can even be a wider form) and this may lead to
    type-unsafe results.  The main reason is to allow for things like
    narrowing ``list[object]`` to ``list[str]`` even though the latter is not
    a subtype of the former, since ``list`` is invariant.  The responsibility of
    writing type-safe type predicates is left to the user.

    ``TypeGuard`` also works with type variables.  For more information, see
    PEP 647 (User-Defined Type Guards).
    """
    item = _type_check(parameters, f'{self} accepts only single type.')
    return _typing._GenericAlias(self, (item,))


@_SpecialForm
def TypeIs(self, parameters):
    """Special typing construct for marking user-defined type predicate functions.

    ``TypeIs`` can be used to annotate the return type of a user-defined
    type predicate function.  ``TypeIs`` only accepts a single type argument.
    At runtime, functions marked this way should return a boolean and accept
    at least one argument.

    ``TypeIs`` aims to benefit *type narrowing* -- a technique used by static
    type checkers to determine a more precise type of an expression within a
    program's code flow.  Usually type narrowing is done by analyzing
    conditional code flow and applying the narrowing to a block of code.  The
    conditional expression here is sometimes referred to as a "type predicate".

    Sometimes it would be convenient to use a user-defined boolean function
    as a type predicate.  Such a function should use ``TypeIs[...]`` or
    ``TypeGuard[...]`` as its return type to alert static type checkers to
    this intention.  ``TypeIs`` usually has more intuitive behavior than
    ``TypeGuard``, but it cannot be used when the input and output types
    are incompatible (e.g., ``list[object]`` to ``list[int]``) or when the
    function does not return ``True`` for all instances of the narrowed type.

    Using  ``-> TypeIs[NarrowedType]`` tells the static type checker that for
    a given function:

    1. The return value is a boolean.
    2. If the return value is ``True``, the type of its argument
       is the intersection of the argument's original type and
       ``NarrowedType``.
    3. If the return value is ``False``, the type of its argument
       is narrowed to exclude ``NarrowedType``.

    For example::

        from typing import assert_type, final, TypeIs

        class Parent: pass
        class Child(Parent): pass
        @final
        class Unrelated: pass

        def is_parent(val: object) -> TypeIs[Parent]:
            return isinstance(val, Parent)

        def run(arg: Child | Unrelated):
            if is_parent(arg):
                # Type of ``arg`` is narrowed to the intersection
                # of ``Parent`` and ``Child``, which is equivalent to
                # ``Child``.
                assert_type(arg, Child)
            else:
                # Type of ``arg`` is narrowed to exclude ``Parent``,
                # so only ``Unrelated`` is left.
                assert_type(arg, Unrelated)

    The type inside ``TypeIs`` must be consistent with the type of the
    function's argument; if it is not, static type checkers will raise
    an error.  An incorrectly written ``TypeIs`` function can lead to
    unsound behavior in the type system; it is the user's responsibility
    to write such functions in a type-safe manner.

    ``TypeIs`` also works with type variables.  For more information, see
    PEP 742 (Narrowing types with ``TypeIs``).
    """
    item = _type_check(parameters, f'{self} accepts only single type.')
    return _typing._GenericAlias(self, (item,))


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
def Concatenate(self, parameters):
    """Special form for annotating higher-order functions.

    ``Concatenate`` can be used in conjunction with ``ParamSpec`` and
    ``Callable`` to represent a higher-order function which adds, removes or
    transforms the parameters of a callable.

    For example::

        Callable[Concatenate[int, P], int]

    See PEP 612 for detailed information.
    """
    if parameters == ():
        raise TypeError("Cannot take a Concatenate of no types.")
    if not isinstance(parameters, tuple):
        parameters = (parameters,)
    if not (parameters[-1] is ... or isinstance(parameters[-1], _typing.ParamSpec)):
        raise TypeError("The last parameter to Concatenate should be a "
                        "ParamSpec variable or ellipsis.")
    msg = "Concatenate[arg, ...]: each arg must be a type."
    parameters = (*(_type_check(p, msg) for p in parameters[:-1]), parameters[-1])
    return _typing._ConcatenateGenericAlias(self, parameters)


def _is_param_expr(arg):
    return arg is ... or isinstance(arg, (tuple, list, _typing.ParamSpec, _typing._ConcatenateGenericAlias))


def _should_unflatten_callable_args(typ, args):
    """Internal helper for munging collections.abc.Callable's __args__.

    The canonical representation for a Callable's __args__ flattens the
    argument types, see https://github.com/python/cpython/issues/86361.

    For example::

        >>> import collections.abc
        >>> P = ParamSpec('P')
        >>> collections.abc.Callable[[int, int], str].__args__ == (int, int, str)
        True
        >>> collections.abc.Callable[P, str].__args__ == (P, str)
        True

    As a result, if we need to reconstruct the Callable from its __args__,
    we need to unflatten it.
    """
    return (
        typ.__origin__ is _ABCCallable
        and not (len(args) == 2 and _is_param_expr(args[0]))
    )


def _paramspec_subst(self, arg):
    if isinstance(arg, (list, tuple)):
        arg = tuple(_type_check(a, "Expected a type.") for a in arg)
    elif not _is_param_expr(arg):
        raise TypeError(f"Expected a list of types, an ellipsis, "
                        f"ParamSpec, or Concatenate. Got {arg}")
    return arg


def _paramspec_prepare_subst(self, alias, args):
    params = alias.__parameters__
    i = params.index(self)
    if i == len(args) and self.has_default():
        args = (*args, self.__default__)
    if i >= len(args):
        raise TypeError(f"Too few arguments for {alias}")
    # Special case where Z[[int, str, bool]] == Z[int, str, bool] in PEP 612.
    if len(params) == 1 and not _is_param_expr(args[0]):
        assert i == 0
        args = (args,)
    # Convert lists to tuples to help other libraries cache the results.
    elif isinstance(args[i], list):
        args = (*args[:i], tuple(args[i]), *args[i+1:])
    return args


def _concatenate_copy_with(self, params):
    if isinstance(params[-1], (list, tuple)):
        return (*params[:-1], *params[-1])
    if isinstance(params[-1], _typing._ConcatenateGenericAlias):
        params = (*params[:-1], *params[-1].__args__)
    return type(self)(self.__origin__, params, name=self._name, inst=self._inst)


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

    def __init__(self, origin, nparams, *, inst=True, name=None, defaults=()):
        if name is None:
            name = origin.__name__
        self.__origin__ = origin
        self._name = name
        self.__qualname__ = name
        self._inst = inst
        self._nparams = nparams
        self._defaults = defaults
        self.__slots__ = None
        module = "" if origin.__module__ == "builtins" else origin.__module__ + "."
        self.__doc__ = f"Deprecated alias to {module}{origin.__qualname__}."

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
        if (attr.startswith("__") and attr.endswith("__")) or attr in ("_name", "_inst", "_nparams", "_defaults"):
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
        missing = self._nparams - len(args)
        if 0 < missing <= len(self._defaults):
            args += self._defaults[-missing:]
        elif missing:
            if not self._nparams:
                raise TypeError(f"{self} is not a generic class")
            expected = f"at least {self._nparams - len(self._defaults)}" if self._defaults else self._nparams
            count = "many" if len(args) > self._nparams else "few"
            raise TypeError(f"Too {count} arguments for {self}; actual {len(args)}, expected {expected}")
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


class _DeprecatedGenericAlias(_SpecialGenericAlias):
    def __init__(
        self, origin, nparams, *, removal_version, inst=True, name=None
    ):
        super().__init__(origin, nparams, inst=inst, name=name)
        self._removal_version = removal_version

    def __instancecheck__(self, inst):
        import warnings
        warnings._deprecated(
            f"{self.__module__}.{self._name}", remove=self._removal_version
        )
        return super().__instancecheck__(inst)


class _TupleType(_SpecialGenericAlias):
    __module__ = __name__

    def _getitem(self, params):
        if not isinstance(params, tuple):
            params = (params,)
        message = "Tuple[t0, t1, ...]: each t must be a type."
        if len(params) >= 2 and params[-1] is ...:
            message = "Tuple[t, ...]: t must be a type."
        return self.copy_with(tuple(_alias_type_arg(p, message) for p in params))


Tuple = _TupleType(tuple, -1, name="Tuple", inst=False)
Tuple.__doc__ = \
    """Deprecated alias to builtins.tuple.

    Tuple[X, Y] is the cross-product type of X and Y.

    Example: Tuple[T1, T2] is a tuple of two elements corresponding
    to type variables T1 and T2.  Tuple[int, float, str] is a tuple
    of an int, a float and a string.

    To specify a variable-length tuple of homogeneous type, use Tuple[T, ...].
    """
Awaitable = _SpecialGenericAlias(_Awaitable, 1)
AbstractSet = _SpecialGenericAlias(_abc.Set, 1, name="AbstractSet")
AsyncGenerator = _SpecialGenericAlias(_abc.AsyncGenerator, 2, defaults=(type(None),))
AsyncIterable = _SpecialGenericAlias(_abc.AsyncIterable, 1)
Collection = _SpecialGenericAlias(_abc.Collection, 1)
Container = _SpecialGenericAlias(_abc.Container, 1)
Coroutine = _SpecialGenericAlias(_abc.Coroutine, 3)
Hashable = _SpecialGenericAlias(_abc.Hashable, 0)
ItemsView = _SpecialGenericAlias(_abc.ItemsView, 2)
KeysView = _SpecialGenericAlias(_abc.KeysView, 1)
MappingView = _SpecialGenericAlias(_abc.MappingView, 1)
MutableSequence = _SpecialGenericAlias(_abc.MutableSequence, 1)
MutableSet = _SpecialGenericAlias(_abc.MutableSet, 1)
ValuesView = _SpecialGenericAlias(_abc.ValuesView, 1)
AsyncIterator = _SpecialGenericAlias(_AsyncIterator, 1)
Generator = _SpecialGenericAlias(_Generator, 3, defaults=(type(None), type(None)))
List = _SpecialGenericAlias(list, 1, name="List", inst=False)
Set = _SpecialGenericAlias(set, 1, name="Set", inst=False)
FrozenSet = _SpecialGenericAlias(frozenset, 1, name="FrozenSet", inst=False)
Dict = _SpecialGenericAlias(dict, 2, name="Dict", inst=False)
Type = _SpecialGenericAlias(type, 1, name="Type", inst=False)
Type.__doc__ = \
    """Deprecated alias to builtins.type.

    builtins.type or typing.Type can be used to annotate class objects.
    For example, suppose we have the following classes::

        class User: ...  # Abstract base for User classes
        class BasicUser(User): ...
        class ProUser(User): ...
        class TeamUser(User): ...

    And a function that takes a class argument that's a subclass of
    User and returns an instance of the corresponding class::

        def new_user[U](user_class: Type[U]) -> U:
            user = user_class()
            # (Here we could write the user object to a database)
            return user

        joe = new_user(BasicUser)

    At this point the type checker knows that joe has type BasicUser.
    """
Deque = _SpecialGenericAlias(_deque, 1, name="Deque")
Counter = _SpecialGenericAlias(_Counter, 1)
DefaultDict = _SpecialGenericAlias(_defaultdict, 2, name="DefaultDict")
OrderedDict = _SpecialGenericAlias(_OrderedDict, 2)
ChainMap = _SpecialGenericAlias(_ChainMap, 2)
Sequence = _SpecialGenericAlias(_Sequence, 1)
Mapping = _SpecialGenericAlias(_Mapping, 2, name="Mapping")
MutableMapping = _SpecialGenericAlias(_MutableMapping, 2, name="MutableMapping")
Iterable = _SpecialGenericAlias(_Iterable, 1, name="Iterable")
Iterator = _SpecialGenericAlias(_abc.Iterator, 1)
Reversible = _SpecialGenericAlias(_Reversible, 1)
Sized = _SpecialGenericAlias(_Sized, 0)
ByteString = _DeprecatedGenericAlias(_ByteString, 0, removal_version=(3, 17))


# Reconstruction callables and arguments used by CPython's typing alias reducers.
def _reduce_generic_alias(alias):
    from operator import getitem
    origin = globals()[alias._name] if alias._name else alias.__origin__
    args = tuple(alias.__args__)
    if len(args) == 1 and not isinstance(args[0], tuple):
        args, = args
    return getitem, (origin, args)


def _reduce_callable_alias(alias):
    from operator import getitem
    args = alias.__args__
    if _should_unflatten_callable_args(alias, args):
        args = list(args[:-1]), args[-1]
    return getitem, (Callable, args)


def _rebuild_builtin_alias(alias, args):
    from types import GenericAlias
    result = GenericAlias(alias.__origin__, args)
    return next(iter(result)) if alias.__unpacked__ else result


def _substitute_alias_arguments(args, replacements):
    result = []
    for arg in args:
        if _is_unpacked_typevartuple(arg):
            result.extend(replacements[arg.__args__[0]])
        else:
            result.append(_substitute_alias_argument(arg, replacements))
    return tuple(result)


def _substitute_alias_argument(arg, replacements):
    if isinstance(arg, type):
        return arg
    for parameter, value in replacements.items():
        if arg is parameter:
            if isinstance(parameter, _typing.TypeVarTuple):
                raise TypeError("Substitution of bare TypeVarTuple is not supported")
            return value
    parameters = getattr(arg, '__parameters__', ())
    if not parameters:
        return arg
    from types import GenericAlias
    if type(arg) is GenericAlias:
        return _rebuild_builtin_alias(arg, _substitute_alias_arguments(arg.__args__, replacements))
    if _typing.get_origin(arg) is _typing.Union:
        return _typing.Union[_substitute_alias_arguments(arg.__args__, replacements)]
    subargs = []
    for parameter in parameters:
        if isinstance(parameter, _typing.TypeVarTuple):
            subargs.extend(replacements[parameter])
        else:
            subargs.append(replacements[parameter])
    return arg[tuple(subargs)]


def _unpack_alias_args(args):
    expanded = []
    for arg in args:
        unpacked = getattr(arg, "__typing_unpacked_tuple_args__", None)
        if unpacked is not None and not (unpacked and unpacked[-1] is ...):
            expanded.extend(unpacked)
        else:
            expanded.append(arg)
    return tuple(expanded)


def _prepare_typevartuple(pack, alias, args):
    # A single pack consumes the arguments between the fixed parameters.
    params = alias.__parameters__
    index = params.index(pack)
    left = index
    right = len(params) - index - 1
    if any(isinstance(p, _typing.TypeVarTuple) for p in params[index + 1:]):
        raise TypeError(f"More than one TypeVarTuple parameter in {alias}")
    unbounded = False
    fill = None
    for i, arg in enumerate(args):
        unpacked = None if isinstance(arg, type) else getattr(arg, "__typing_unpacked_tuple_args__", None)
        if unpacked and len(unpacked) == 2 and unpacked[-1] is ...:
            if unbounded:
                raise TypeError("More than one unpacked arbitrary-length tuple argument")
            unbounded = True
            fill = unpacked[0]
            left = min(left, i)
            right = min(right, len(args) - i - 1)
    if not unbounded and len(args) < left + right:
        raise TypeError(f"Too few arguments for {alias}; actual {len(args)}, expected at least {len(params) - 1}")
    end = len(args) - right
    grouped = args[left:end]
    if not grouped and pack.has_default():
        grouped = _unpack_alias_args((pack.__default__,))
    return (args[:left] + (fill,) * (index - left) + (tuple(grouped),)
            + (fill,) * (len(params) - right - left - index - 1) + args[end:])


def _specialize_generic_alias_uncached(alias, args):
    from types import GenericAlias
    if type(alias) is not GenericAlias and (alias.__origin__ is Generic or alias.__origin__ is Protocol):
        raise TypeError(f"Cannot subscript already-subscripted {alias}")
    parameters = alias.__parameters__
    if not parameters:
        raise TypeError(f"{alias} is not a generic class")
    if not isinstance(args, tuple):
        args = (args,)
    if type(alias) is not GenericAlias:
        args = tuple(_type_convert(arg) for arg in args)
    expanded = _unpack_alias_args(args)
    for parameter in parameters:
        if isinstance(parameter, _typing.TypeVarTuple):
            expanded = _prepare_typevartuple(parameter, alias, expanded)
        elif isinstance(parameter, _typing.ParamSpec):
            expanded = parameter.__typing_prepare_subst__(alias, expanded)
    if len(expanded) != len(parameters):
        which = "few" if len(expanded) < len(parameters) else "many"
        expected = f"at least {len(expanded) + 1}" if which == "few" else str(len(parameters))
        raise TypeError(f"Too {which} arguments for {alias}; actual {len(expanded)}, expected {expected}")
    replacements = {}
    for parameter, arg in zip(parameters, expanded):
        if isinstance(parameter, _typing.TypeVarTuple):
            replacements[parameter] = arg
            continue
        if isinstance(parameter, _typing.ParamSpec):
            replacements[parameter] = parameter.__typing_subst__(arg)
            continue
        arg = _alias_type_arg(arg)
        if _typing.get_origin(arg) is Unpack:
            raise TypeError(f"{arg} is not valid as type argument")
        replacements[parameter] = arg
    args = _substitute_alias_arguments(alias.__args__, replacements)
    if type(alias) is GenericAlias:
        return _rebuild_builtin_alias(alias, args)
    if isinstance(alias, _typing.Union):
        return _typing.Union[args]
    if isinstance(alias, (_AnnotatedAlias, _typing._ConcatenateGenericAlias)):
        return alias.copy_with(args)
    if type(alias) is _typing._CallableGenericAlias:
        # typing flattens each substituted tuple, even one in the result position.
        flat = tuple(item for arg in args for item in (arg if isinstance(arg, tuple) else (arg,)))
        return _typing._CallableGenericAlias(alias.__origin__, flat, name="Callable")
    if alias.__origin__ is _typing.get_origin(Callable):
        # collections.abc.Callable rebuilds its argument list separately from its result.
        if not isinstance(args[0], (tuple, list)):
            args = (args[:-1], args[-1])
        return alias.__origin__[args]
    return _typing._GenericAlias(alias.__origin__, args, name=alias._name, inst=alias._inst)


_cached_generic_specialization = lru_cache(_specialize_generic_alias_uncached)


def _specialize_generic_alias(alias, args):
    try:
        return _cached_generic_specialization(alias, args)
    except TypeError:
        return _specialize_generic_alias_uncached(alias, args)


class _TypedCacheSpecialForm(_SpecialForm, _root=True):
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


# PEP 593 stores metadata separately from the wrapped type's generic arguments.
class _AnnotatedAlias(_NotIterable):
    __module__ = __name__
    _name = "Annotated"
    _inst = True
    __getattr__ = _SpecialGenericAlias.__getattr__
    __setattr__ = _SpecialGenericAlias.__setattr__
    __call__ = _SpecialGenericAlias.__call__
    __or__ = _SpecialGenericAlias.__or__
    __ror__ = _SpecialGenericAlias.__ror__

    def __init__(self, origin, metadata):
        if isinstance(origin, _AnnotatedAlias):
            metadata = origin.__metadata__ + metadata
            origin = origin.__origin__
        self.__origin__ = origin
        self.__args__ = (origin,)
        self.__metadata__ = metadata
        if isinstance(origin, (TypeVar, _typing.TypeVarTuple)):
            self.__parameters__ = (origin,)
        else:
            self.__parameters__ = () if isinstance(origin, type) else getattr(origin, "__parameters__", ())

    def __repr__(self):
        origin = _annotation_type_repr(self.__origin__)
        return f"typing.Annotated[{origin}, {', '.join(repr(item) for item in self.__metadata__)}]"

    def __eq__(self, other):
        if not isinstance(other, _AnnotatedAlias):
            return NotImplemented
        return self.__origin__ == other.__origin__ and self.__metadata__ == other.__metadata__

    def __hash__(self):
        return hash((self.__origin__, self.__metadata__))

    def copy_with(self, params):
        assert len(params) == 1
        return _AnnotatedAlias(params[0], self.__metadata__)

    def __reduce__(self):
        from operator import getitem
        return getitem, (Annotated, (self.__origin__,) + self.__metadata__)

    def __getitem__(self, parameters):
        if isinstance(self.__origin__, _typing.TypeVarTuple):
            raise TypeError("Substitution of bare TypeVarTuple is not supported")
        return _specialize_generic_alias(self, parameters)

    def __mro_entries__(self, bases):
        return (self.__origin__,)

    def __subclasscheck__(self, cls):
        raise TypeError("Subscripted generics cannot be used with class and instance checks")

    __instancecheck__ = __subclasscheck__


@_TypedCacheSpecialForm
def Annotated(self, *parameters):
    """Attach ordered metadata to a type without changing its underlying type."""
    if len(parameters) < 2:
        raise TypeError("Annotated[...] should be used with at least two arguments (a type and an annotation).")
    origin = parameters[0]
    if not isinstance(origin, type) and getattr(origin, "__typing_is_unpacked_typevartuple__", False) is True:
        raise TypeError("Annotated[...] should not be used with an unpacked TypeVarTuple")
    origin = _type_check(origin, "Annotated[t, ...]: t must be a type.", allow_special_forms=True)
    if isinstance(origin, str):
        origin = _make_forward_ref(origin, is_class=True)
    return _AnnotatedAlias(type(None) if origin is None else origin, parameters[1:])


@_TypedCacheSpecialForm
def Literal(self, *parameters):
    """A type whose arguments are literal values."""
    return _typing._LiteralGenericAlias(self, parameters)


# VALUE-format annotation resolution from CPython 3.14.5 Lib/typing.py.
def _hint_scope(params, globalns, localns, is_class):
    if params:
        globalns = dict(globalns)
        localns = dict(localns)
        for param in params:
            if not is_class or param.__name__ not in globalns:
                globalns[param.__name__] = param
                localns.pop(param.__name__, None)
    return globalns, localns


def _resolve_hint(value, globalns, localns, owner, seen, include_extras, format, parent_ref=None):
    from annotationlib import Format, ForwardRef
    from types import GenericAlias, ModuleType
    if isinstance(value, str):
        value = ForwardRef(value, is_argument=not isinstance(owner, (type, ModuleType)), is_class=isinstance(owner, type))
    if isinstance(value, ForwardRef):
        expr = value.__forward_arg__
        if expr in seen:
            return value
        if value.__forward_module__ is not None:
            import sys
            module = sys.modules.get(value.__forward_module__)
            if module is not None:
                globalns = module.__dict__
        result = value.evaluate(globals=globalns, locals=localns, type_params=(), owner=owner, format=format)
        if format == Format.FORWARDREF and isinstance(result, ForwardRef):
            return result
        if isinstance(result, str):
            result = ForwardRef(result, module=value.__forward_module__, owner=owner or value.__owner__,
                                is_argument=value.__forward_is_argument__, is_class=value.__forward_is_class__)
        return _resolve_hint(result, globalns, localns, owner, seen + (expr,), include_extras, format, value)

    origin = _typing.get_origin(value)
    if origin is None or origin is Literal:
        return value
    args = getattr(value, '__args__', ())
    values = args
    if type(value) is GenericAlias:
        # Strings inside builtin aliases are type arguments, even in class annotations.
        module = getattr(parent_ref, '__forward_module__', None)
        ref_owner = getattr(parent_ref, '__owner__', None)
        values = tuple(ForwardRef(arg, module=module, owner=ref_owner) if isinstance(arg, str) else arg for arg in args)
    resolved = tuple(_resolve_hint(a, globalns, localns, owner, seen, include_extras, format) for a in values)
    if isinstance(value, _AnnotatedAlias):
        if not include_extras:
            return resolved[0]
        return value if resolved == args else value.copy_with(resolved)
    if not include_extras:
        for name in ('Required', 'NotRequired', 'ReadOnly'):
            if origin is getattr(_typing, name, None):
                return resolved[0]
    if type(value) is GenericAlias and value.__unpacked__:
        return Unpack[GenericAlias(origin, resolved)]
    if resolved == args:
        return value
    if origin is _typing.Union:
        return _typing.Union[resolved]
    if type(value) is GenericAlias:
        return GenericAlias(origin, resolved)
    if type(value) is _typing._GenericAlias:
        name = getattr(value, '_name', None)
        return _typing._GenericAlias(origin, resolved, name=name, inst=getattr(value, '_inst', True))
    return type(value)(origin, resolved)


def get_type_hints(obj, globalns=None, localns=None, include_extras=False, *, format=None):
    """Resolve annotations in the defining namespace, including inherited class annotations."""
    if getattr(obj, '__no_type_check__', None):
        return {}
    from annotationlib import Format, get_annotations
    if format is None:
        format = Format.VALUE
    if isinstance(obj, type):
        import sys
        hints = {}
        for base in reversed(getattr(obj, '__mro__', (obj,))):
            ann = get_annotations(base, format=format)
            if format == Format.STRING:
                hints.update(ann)
                continue
            if not ann:
                continue
            base_globals = globalns
            if base_globals is None:
                module = sys.modules.get(base.__module__)
                base_globals = getattr(module, '__dict__', {})
            base_locals = dict(base.__dict__) if localns is None else localns
            if localns is None and globalns is None:
                base_globals, base_locals = base_locals, base_globals
            base_globals, base_locals = _hint_scope(getattr(base, '__type_params__', ()),
                                                    base_globals, base_locals, True)
            for name, value in ann.items():
                result = _resolve_hint(value, base_globals, base_locals, obj, (), include_extras, format)
                hints[name] = type(None) if result is None else result
        return hints

    hints = get_annotations(obj, format=format)
    if format == Format.STRING:
        return hints
    if globalns is None:
        from types import ModuleType
        if isinstance(obj, ModuleType):
            globalns = obj.__dict__
        else:
            wrapped = obj
            seen = {id(wrapped)}
            while hasattr(wrapped, '__wrapped__'):
                wrapped = wrapped.__wrapped__
                if id(wrapped) in seen:
                    raise ValueError(f'wrapper loop when unwrapping {obj!r}')
                seen.add(id(wrapped))
            globalns = getattr(wrapped, '__globals__', {})
    if localns is None:
        localns = globalns
    globalns, localns = _hint_scope(getattr(obj, '__type_params__', ()), globalns, localns, False)
    for name, value in hints.items():
        result = _resolve_hint(value, globalns, localns, obj, (), include_extras, format)
        hints[name] = type(None) if result is None else result
    return hints


# PEP 681 decorator from CPython 3.14.5 Lib/typing.py. See stdlib/docstrings/LICENSE.
class _IdentityCallable(Protocol):
    def __call__[T](self, arg: T, /) -> T:
        ...


def dataclass_transform(
    *,
    eq_default: _builtins.bool = True,
    order_default: _builtins.bool = False,
    kw_only_default: _builtins.bool = False,
    frozen_default: _builtins.bool = False,
    field_specifiers: tuple[type[Any] | Callable[..., Any], ...] = (),
    **kwargs: Any,
) -> _IdentityCallable:
    """Decorator to mark an object as providing dataclass-like behaviour.

    The decorator can be applied to a function, class, or metaclass.

    Example usage with a decorator function::

        @dataclass_transform()
        def create_model[T](cls: type[T]) -> type[T]:
            ...
            return cls

        @create_model
        class CustomerModel:
            id: int
            name: str

    On a base class::

        @dataclass_transform()
        class ModelBase: ...

        class CustomerModel(ModelBase):
            id: int
            name: str

    On a metaclass::

        @dataclass_transform()
        class ModelMeta(type): ...

        class ModelBase(metaclass=ModelMeta): ...

        class CustomerModel(ModelBase):
            id: int
            name: str

    The ``CustomerModel`` classes defined above will
    be treated by type checkers similarly to classes created with
    ``@dataclasses.dataclass``.
    For example, type checkers will assume these classes have
    ``__init__`` methods that accept ``id`` and ``name``.

    The arguments to this decorator can be used to customize this behavior:
    - ``eq_default`` indicates whether the ``eq`` parameter is assumed to be
        ``True`` or ``False`` if it is omitted by the caller.
    - ``order_default`` indicates whether the ``order`` parameter is
        assumed to be True or False if it is omitted by the caller.
    - ``kw_only_default`` indicates whether the ``kw_only`` parameter is
        assumed to be True or False if it is omitted by the caller.
    - ``frozen_default`` indicates whether the ``frozen`` parameter is
        assumed to be True or False if it is omitted by the caller.
    - ``field_specifiers`` specifies a static list of supported classes
        or functions that describe fields, similar to ``dataclasses.field()``.
    - Arbitrary other keyword arguments are accepted in order to allow for
        possible future extensions.

    At runtime, this decorator records its arguments in the
    ``__dataclass_transform__`` attribute on the decorated object.
    It has no other runtime effect.

    See PEP 681 for more details.
    """
    def decorator(cls_or_fn):
        cls_or_fn.__dataclass_transform__ = {
            "eq_default": eq_default,
            "order_default": order_default,
            "kw_only_default": kw_only_default,
            "frozen_default": frozen_default,
            "field_specifiers": field_specifiers,
            "kwargs": kwargs,
        }
        return cls_or_fn
    return decorator


def __getattr__(attr):
    if attr in {"Pattern", "Match"}:
        import re
        alias = _SpecialGenericAlias(getattr(re, attr), 1)
    elif attr in {"ContextManager", "AsyncContextManager"}:
        import contextlib
        alias = _SpecialGenericAlias(getattr(contextlib, f"Abstract{attr}"), 2, name=attr, defaults=(bool | None,))
    else:
        raise AttributeError(f"module {__name__!r} has no attribute {attr!r}")
    globals()[attr] = alias
    return alias
