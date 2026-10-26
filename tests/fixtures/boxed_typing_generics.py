from sys import path
from types import GenericAlias
import typing as t


K = t.TypeVar("K")
V = t.TypeVar("V")
T = t.TypeVar("T", bound=str, covariant=True)
print(K.__name__, K.__module__, K.__bound__ is None, K.__constraints__)
print(K is t.TypeVar("K"), T.__bound__ is str, T.__covariant__, t.TYPE_CHECKING)


class Accessor(t.Generic[T]):
    pass


class DictMixin(dict[K, V]):
    pass


class CallbackDict(DictMixin[K, V], dict[K, V]):
    pass


alias = Accessor.__orig_bases__[0]
print(alias.__origin__ is t.Generic, alias is t.Generic[T], alias.__args__[0] is T)
print(Accessor.__parameters__[0] is T, Accessor.__bases__ == (t.Generic,))
print(Accessor.__mro__ == (Accessor, t.Generic, object))
mapping = DictMixin.__orig_bases__[0]
print(type(mapping) is GenericAlias, GenericAlias(dict, (K, V)) == mapping)
print(mapping.__origin__ is dict, mapping.__args__ == (K, V), mapping.__parameters__ == (K, V))
print(mapping is dict[K, V], mapping == dict[K, V], mapping.__mro_entries__((mapping,)) == (dict,))
print(CallbackDict.__bases__ == (DictMixin, dict), CallbackDict.__mro__ == (CallbackDict, DictMixin, dict, object))
print(t.get_origin(CallbackDict.__orig_bases__[0]) is DictMixin, t.get_args(CallbackDict.__orig_bases__[1]) == (K, V))


class Left:
    @t.overload
    def select(self, arg):
        return "left one"

    @t.overload
    def select(self, arg):
        return "left two"

    def select(self, arg):
        return "implementation"


class Right:
    @t.overload
    def select(self, arg):
        return "right"

    def select(self, arg):
        return "other"


old = t.get_overloads(Left.select)
print(len(old), old[0](None, 1), old[1](None, 2), old[0] is t.get_overloads(Left().select)[0])
print(len(t.get_overloads(Right.select)), t.get_overloads(Right.select)[0](None, 3))
t.clear_overloads()
print(t.get_overloads(Left.select), Left().select(None))

marker = []
print(t.cast(T, marker) is marker, t.get_origin(t.Generic) is t.Generic)

print(type(K) is t.TypeVar, K.__class__ is type(K), isinstance(K, t.TypeVar))
print(type(mapping).__name__, type(mapping).__module__, type(mapping) is type(list[T]))
print(type(alias).__name__, type(alias).__module__, alias.__class__ is type(Accessor[V]))
print(t.Generic.__bases__ == (object,), t.Generic.__mro__ == (t.Generic, object))
print(CallbackDict.__bases__ is CallbackDict.__bases__, Accessor.__mro__ is Accessor.__mro__)
print(t.cast(val=marker, typ=K) is marker)


class Other(t.Generic[T]):
    pass


class Composite(t.Generic[T], Other[T]):
    pass


print(Composite.__mro__ == (Composite, Other, t.Generic, object), Composite.__parameters__ == (T,))
print(Composite.__orig_bases__[0].__mro_entries__(Composite.__orig_bases__) == ())


class BaseHook:
    def __mro_entries__(self, bases):
        self.seen = bases
        return (dict,)


hook = BaseHook()


class Based(hook):
    pass


print(hook.seen is Based.__orig_bases__, Based.__bases__ == (dict,))

import collections.abc as cabc

bound = t.TypeVar("F", bound=cabc.Callable[..., t.Any])
print(bound.__bound__.__origin__ is cabc.Callable, type(bound.__bound__).__module__)
print(t.get_origin(t.Callable) is cabc.Callable, type(t.Callable[..., int]).__module__)
print(isinstance(lambda: None, cabc.Callable), isinstance(1, cabc.Callable))
print(repr(t.Generic[K]), repr(dict[K,V]))
print(repr(bound.__bound__), repr(t.Callable[[int, str], bytes]))
print(t.get_args(t.Callable[[int, str], bytes]) == ([int, str], bytes))
print(dict[K,V]() == {}, Accessor[int]().__orig_class__ is Accessor[int])

inferred = t.TypeVar("Inferred", infer_variance=True)
print(K.__infer_variance__, inferred.__infer_variance__, repr(inferred))
for invalid in (
    lambda: t.TypeVar(1),
    lambda: t.TypeVar("One", int),
    lambda: t.TypeVar("Both", covariant=True, infer_variance=True),
    lambda: t.Generic[int],
    lambda: t.Generic[K, K],
    lambda: t.Generic[()],
):
    try:
        invalid()
    except (TypeError, ValueError) as error:
        print(type(error).__name__, str(error))

print(Accessor[list[int]] is Accessor[list[int]])

print(repr(t.Any), repr(t.Callable), isinstance(lambda: 1, t.Callable), isinstance(1, (int, K)))
for classinfo in (K, dict[K, V], t.Generic[K], cabc.Callable[..., int], t.Any, (K, int)):
    try:
        isinstance(1, classinfo)
    except TypeError as error:
        print(str(error))
for base in (K, t.TypeVar):
    try:
        class Invalid(base):
            pass
    except TypeError as error:
        print(str(error))


class Parameterize:
    def __get__(self, instance, owner):
        def bind(args):
            return type(list[int])(owner, args)
        return bind


class Parameterized:
    __class_getitem__ = Parameterize()


class Child(Parameterized):
    pass


params = K, V
applied = Child[params]


class Consumer(applied):
    pass


print(applied.__origin__ is Child, applied.__args__ is params, applied.__parameters__ == params)
print(Consumer.__bases__ == (Child,), Consumer.__mro__ == (Consumer, Child, Parameterized, object))
print(Consumer.__orig_bases__[0] is applied, dict[params].__args__ is params)
for invalid in (lambda: type(applied)(Child), lambda: type(applied)(origin=Child, args=params)):
    try:
        invalid()
    except TypeError as error:
        print(str(error))
