import sys
from sys import path
from inspect import signature
from types import FunctionType, GenericAlias
from typing import cast
import typing as t

rendered_spec = t.ParamSpec("P")
assert repr((rendered_spec, rendered_spec.args, rendered_spec.kwargs)) == "(~P, P.args, P.kwargs)"
from typing import BinaryIO, KT, TextIO, VT
from typing import T as typing_T


assert typing_T is t.T and KT is t.KT and VT is t.VT
assert (repr(typing_T), repr(KT), repr(VT)) == ("~T", "~KT", "~VT")

K = t.TypeVar("K")
print(TextIO is t.TextIO, BinaryIO is t.BinaryIO)
print(TextIO.__bases__ == (t.IO,), BinaryIO.__orig_bases__ == (t.IO[bytes],))
print(t.IO.__parameters__ == (t.AnyStr,), TextIO.__parameters__, BinaryIO.__parameters__)
V = t.TypeVar("V")
T = t.TypeVar("T", bound=str, covariant=True)
assert t.TypeVar("Forward", bound="Accessor").__bound__.__forward_arg__ == "Accessor"
try:
    t.TypeVar("Invalid", int, bound=(1, 2))
except TypeError as error:
    assert str(error) == "Bound must be a type. Got (1, 2)."
else:
    raise AssertionError("invalid TypeVar bound was accepted")
print(K.__name__, K.__module__, K.__bound__ is None, K.__constraints__)
print(K is t.TypeVar("K"), T.__bound__ is str, T.__covariant__, t.TYPE_CHECKING)


class Accessor(t.Generic[T]):
    pass


def local_alias_names():
    class C(t.Generic[K]):
        pass

    class N(int):
        pass

    assert N.from_bytes.__qualname__ == N.__qualname__ + ".from_bytes"
    assert C.__init_subclass__.__qualname__ == C.__qualname__ + ".__init_subclass__"

    alias = C[int]
    assert repr(alias) == f"{__name__}.local_alias_names.<locals>.C[int]"
    assert alias.__module__ == __name__ and "__module__" in dir(alias)
    assert alias.__name__ == alias.__qualname__ == "C"
    C.__name__ = "Moved"
    C.__module__ = "other"
    assert alias.__module__ == __name__ and C[str].__module__ == "other"
    assert C[int] is alias and alias.__origin__ is C and alias.__args__ == (int,)
    assert alias.__name__ == alias.__qualname__ == "Moved"
    marker = object()
    for name in ("__module__", "__name__", "__qualname__"):
        setattr(alias, name, marker)
        assert getattr(alias, name) is marker and name in dir(alias)
        delattr(alias, name)
        assert getattr(alias, name) == ("typing" if name == "__module__" else "Moved")
        try:
            delattr(alias, name)
        except AttributeError:
            pass
        else:
            raise AssertionError("deleted an absent alias field")
    assert C.__module__ == "other" and C[str].__name__ == "Moved"


local_alias_names()


class GenericHook:
    def __init_subclass__(cls, *, initialize):
        assert "__parameters__" not in vars(cls)
        if initialize:
            super().__init_subclass__()


class Initialized(GenericHook, t.Generic[K], initialize=True):
    pass


class Uninitialized[U](GenericHook, initialize=False):
    pass


assert Initialized.__parameters__ == (K,) and t.get_origin(Initialized[int]) is Initialized
try:
    Uninitialized[int]
except AttributeError as error:
    assert error.__notes__ == [
        "Note: this exception may have been caused by 'GenericHook.__init_subclass__' "
        "(or the '__init_subclass__' method on a superclass) not calling 'super().__init_subclass__()'"
    ]
else:
    raise AssertionError("subscribed a class whose hook skipped Generic initialization")


class Slotted(t.Generic[T]):
    __slots__ = ("value",)


assert "__slots__" not in vars(t.Generic)
for obj in (Slotted(), Slotted[int]()):
    obj.value = 7
    assert obj.value == 7 and not hasattr(obj, "__dict__")
assert hasattr(Accessor[int](), "__dict__")


# Parameterizations share class attributes through their common origin.
Accessor[int].marker = object()
assert Accessor[str].marker is Accessor.marker
try:
    del Accessor[int].marker
except AttributeError:
    pass
else:
    raise AssertionError("the attribute belongs to the origin")
assert Accessor[int].marker is Accessor.marker
del Accessor.marker
Accessor[int].copy_with = "origin attribute"
assert Accessor.copy_with == "origin attribute"
assert Accessor[int].copy_with((str,)) == Accessor[str]


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


registry = t._overload_registry
try:
    t._overload_registry = type(registry)(registry.default_factory)
    def temporary(x): pass
    assert t.overload(temporary) is t._overload_dummy
    assert t._overload_registry[__name__][temporary.__qualname__] == {temporary.__code__.co_firstlineno: temporary}
    assert t.get_overloads(temporary) == [temporary]
    assert t.get_overloads(Left.select) == []
    t.clear_overloads()
    assert t._overload_registry == {}
finally:
    t._overload_registry = registry

old = t.get_overloads(Left.select)
print(len(old), old[0](None, 1), old[1](None, 2), old[0] is t.get_overloads(Left().select)[0])
print(len(t.get_overloads(Right.select)), t.get_overloads(Right.select)[0](None, 3))
t.clear_overloads()
print(t.get_overloads(Left.select), Left().select(None))

def first():
    pass


def second():
    pass


second.__qualname__ = first.__qualname__
t.overload(first)
t.overload(second)
second.__code__ = first.__code__
wrapped = staticmethod(second)
t.overload(wrapped)
print(t.get_overloads(first)[0] is wrapped, t.get_overloads(wrapped)[1] is second)
t.clear_overloads()

marker = []
assert cast is t.cast and type(cast) is FunctionType
assert cast.__module__ == "typing" and cast.__qualname__ == "cast"
assert str(signature(cast)) == "(typ, val)"
assert cast(marker, val=marker) is marker
for args, kwargs in (((), {}), ((T,), {}), ((T, marker, None), {}),
                     ((T, marker), {"typ": T}), ((T,), {"other": marker})):
    try:
        cast(*args, **kwargs)
    except TypeError:
        pass
    else:
        raise AssertionError("cast accepted invalid arguments")
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


class BadBaseHook:
    def __mro_entries__(self, bases):
        return (object(),)


for bad_base in (object(), BadBaseHook()):
    ran = []
    try:
        class Invalid(t.List[int], bad_base):
            ran.append(True)
    except TypeError as error:
        assert str(error) == "bases must be types"
    else:
        raise AssertionError("accepted a non-type base")
    assert ran == [True]



class OtherMeta(type):
    pass


ran = []
try:
    class InvalidTuple(t.NamedTuple, metaclass=OtherMeta):
        ran.append(True)
except TypeError as error:
    assert "metaclass conflict" in str(error)
else:
    raise AssertionError("accepted incompatible NamedTuple metaclasses")
assert not ran

import collections.abc as cabc


class Callback(t.Generic[rendered_spec, K]):
    pass


callback = Callback[[int, list[str]], bool]
assert callback.__args__ == ((int, list[str]), bool)
assert repr(callback) == f"{__name__}.Callback[[int, list[str]], bool]"
assert repr(Callback[[], bool]) == f"{__name__}.Callback[[], bool]"
assert repr(t.Literal[((int, (str, bool)),)]) == "typing.Literal[[int, [str, bool]]]"


class ArgumentTuple(tuple):
    def __iter__(self):
        return iter((float,))


assert repr(t.Literal[(ArgumentTuple((int,)),)]) == "typing.Literal[[float]]"
assert repr(list[((int, str),)]) == "list[(<class 'int'>, <class 'str'>)]"
assert repr(cabc.Callable[..., ()]) == "collections.abc.Callable[..., ()]"

nested = int
for _ in range(18):
    nested = t.Literal[((nested,),)]
saved_limit = sys.getrecursionlimit()
try:
    sys.setrecursionlimit(80)
    try:
        repr(nested)
    except RecursionError:
        pass
    else:
        raise AssertionError("tuple arguments lost their enclosing alias frames")
    assert repr(callback) == f"{__name__}.Callback[[int, list[str]], bool]"
finally:
    sys.setrecursionlimit(saved_limit)


class PlainCallable(t.Callable):
    pass


print(PlainCallable.__orig_bases__ == (t.Callable,))
print(PlainCallable.__bases__ == (cabc.Callable, t.Generic))
print(PlainCallable.__mro__ == (PlainCallable, cabc.Callable, t.Generic, object))
print(t.Callable.__mro_entries__((t.Callable,)) == (cabc.Callable, t.Generic))


class EqualBase:
    def __init__(self, target, error=None):
        self.target = target
        self.error = error

    def __eq__(self, other):
        if other is self.target:
            if self.error is not None:
                raise RuntimeError(self.error)
            return True
        return False


equal_origin = EqualBase(cabc.Callable)
print(t.Callable.__mro_entries__((equal_origin, t.Callable)) == (t.Generic,))
equal_callable = EqualBase(t.Callable)
print(t.Callable.__mro_entries__((equal_callable, t.Callable)) == (cabc.Callable,))
for target, message in ((cabc.Callable, "origin equality"), (t.Callable, "self equality")):
    try:
        t.Callable.__mro_entries__((EqualBase(target, message), t.Callable))
    except RuntimeError as error:
        print(str(error))


class AddsGeneric:
    def __mro_entries__(self, bases):
        return (t.Generic,)


adds_generic = AddsGeneric()
print(t.Callable.__mro_entries__((t.Callable, adds_generic)) == (cabc.Callable,))


class CallableWithHook(t.Callable, adds_generic):
    pass


print(CallableWithHook.__bases__ == (cabc.Callable, t.Generic))

bound = t.TypeVar("F", bound=cabc.Callable[..., t.Any])
print(bound.__bound__.__origin__ is cabc.Callable, type(bound.__bound__).__module__)
print(t.get_origin(t.Callable) is cabc.Callable, type(t.Callable[..., int]).__module__)
print(isinstance(lambda: None, cabc.Callable), isinstance(1, cabc.Callable))
print(repr(t.Generic[K]), repr(dict[K,V]))
print(repr(bound.__bound__), repr(t.Callable[[int, str], bytes]))
print(t.get_args(t.Callable[[int, str], bytes]) == ([int, str], bytes))
print(dict[K,V]() == {}, Accessor[int]().__orig_class__ is Accessor[int])


class Factory(t.Generic[K]):
    def __new__(cls, result):
        return result


class Product:
    pass


def decode(value):
    return int(value)


for alias in (Factory[int], GenericAlias(Factory, int)):
    for product in (decode, Product):
        assert alias(product) is product
        assert product.__orig_class__ is alias

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


class Subscription:
    def __class_getitem__(cls, item):
        return cls, item


class InheritedSubscription(Subscription):
    pass


class FallbackMeta(type):
    def __getattr__(cls, name):
        if name == "__class_getitem__":
            return lambda item: (cls, item)
        raise AttributeError(name)


class VirtualSubscription(metaclass=FallbackMeta):
    pass


class ListSubscription(list, metaclass=FallbackMeta):
    pass


assert VirtualSubscription[int] == (VirtualSubscription, int)
assert ListSubscription[int].__origin__ is ListSubscription
assert isinstance(Subscription.__dict__["__class_getitem__"], classmethod)
assert InheritedSubscription[int] == (InheritedSubscription, int)


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


class Pair(t.Generic[K, V]):
    pass


repeat = Pair[K, K]
nested = repeat[t.Tuple[V, K]]
assert nested.__parameters__ == (V, K)
assert nested[int, str] == Pair[t.Tuple[int, str], t.Tuple[int, str]]
assert nested[int, str] is nested[int, str]
assert Pair[V, K][K, V].__args__ == (K, V)
assert Pair[Pair, list[K]][bytes].__args__ == (Pair, list[bytes])
assert Pair[K | int, V][str, None].__args__ == (str | int, type(None))

for alias, args, message in (
    (t.Generic[K], int, f"Cannot subscript already-subscripted {t.Generic[K]}"),
    (t.Protocol[K], int, f"Cannot subscript already-subscripted {t.Protocol[K]}"),
    (Pair[K, V], (), f"Too few arguments for {Pair[K, V]}; actual 0, expected at least 1"),
    (Pair[K, V], (t.Final, t.Unpack[t.TypeVarTuple("Ts")]), "Plain typing.Final is not valid as type argument"),
    (repeat, (int, t.Generic), f"Too many arguments for {repeat}; actual 2, expected 1"),
    (Pair[int, str], bytes, f"{Pair[int, str]} is not a generic class"),
):
    try:
        alias[args]
    except TypeError as error:
        assert str(error) == message
    else:
        raise AssertionError("invalid specialization accepted")


def fresh_box():
    class Box[Item]:
        value: Item

        def parameter(self):
            return Item

        def shadow(self, Item):
            def change():
                nonlocal Item
                Item = str
            change()
            return Item

    return Box


box = fresh_box()
item, = box.__type_params__
assert item is not fresh_box().__type_params__[0]
assert box.__annotations__["value"] is box().parameter() is item
assert box.parameter.__code__.co_freevars == ("Item",)
assert box.parameter.__closure__[0].cell_contents is item
assert box().shadow(bytes) is str and box().parameter() is item
assert item.__name__ == "Item" and item.__infer_variance__ is True
assert "Item" not in box.__dict__ and "Item" not in globals()
assert box.__bases__ == (t.Generic,) and box.__orig_bases__ == (t.Generic[item],)
assert box.__parameters__ == (item,) and t.get_args(box[int]) == (int,)


def fresh_variadic_class():
    class Variadic[Item, *Items, **Signature]:
        def parameters(self):
            return Item, Items, Signature

    return Variadic


variadic = fresh_variadic_class()
item, pack, signature = variadic.__type_params__
assert type(pack) is t.TypeVarTuple and type(signature) is t.ParamSpec
assert variadic.__parameters__ == variadic().parameters() == (item, pack, signature)
assert all(left is not right for left, right in zip(variadic.__type_params__, fresh_variadic_class().__type_params__))
assert (pack.__name__, signature.__name__) == ("Items", "Signature")
assert pack.__module__ == signature.__module__ == "typing"
assert signature.__infer_variance__ and signature.__bound__ is None
assert pack.__default__ is signature.__default__ is t.NoDefault
assert pack.evaluate_default is signature.evaluate_default is None
assert t.get_origin(signature.args) is t.get_origin(signature.kwargs) is signature
original, = variadic.__orig_bases__
assert t.get_origin(original) is t.Generic
first, unpacked, last = t.get_args(original)
assert first is item and last is signature
assert t.get_origin(unpacked) is t.Unpack and t.get_args(unpacked) == (pack,)
assert t.TypeVarTuple("Manual").evaluate_default(1) is t.NoDefault


class HeaderOwner[T]:
    T = "class"

    class Inner[U](list[(T for _ in (0,)), T]):
        pass


iterator, direct = HeaderOwner.Inner.__orig_bases__[0].__args__
assert iterator.gi_code.co_freevars == ("T",)
assert next(iterator) is HeaderOwner.__type_params__[0]
assert direct == "class"


class Hidden[__P]:
    value = __P


assert Hidden.value is Hidden.__type_params__[0] and Hidden.value.__name__ == "__P"


P = t.ParamSpec("P", bound="Hidden", default=(int, str))
assert P.__module__ == __name__ and P.__bound__.__forward_arg__ == "Hidden"
assert P.has_default() and P.__default__ == (int, str)
assert P.args.__origin__ is P and P.kwargs.__origin__ is P
assert P.args == t.ParamSpecArgs(P) and P.kwargs == t.ParamSpecKwargs(P)
assert P.args is not P.args and P.args != P.kwargs
assert P != t.ParamSpec("P") and repr(P.args) == "P.args"
try:
    P.args.__origin__ = None
except AttributeError as error:
    assert str(error) == "readonly attribute"
else:
    raise AssertionError("ParamSpecArgs origin was writable")
