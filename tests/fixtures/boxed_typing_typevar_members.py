import sys
from collections.abc import Callable as ABCCallable
from inspect import getattr_static
from types import MappingProxyType, ModuleType
from typing import Any, Callable, List, TypeVar, Unpack


def current():
    variable = TypeVar("T", covariant=True)
    namespace = vars(TypeVar)
    assert namespace["__reduce__"](variable) == variable.__reduce__() == "T"
    for args in ((), (1,), (variable, 1)):
        try:
            TypeVar.__reduce__(*args)
        except TypeError:
            pass
        else:
            raise AssertionError("invalid TypeVar reducer call")
    assert issubclass(type(Any), type)
    assert Callable.__qualname__ == Callable.__name__ == "Callable"
    assert Callable.__origin__ is Callable[[int], str].__origin__ is ABCCallable
    assert type(Callable).__reduce__ is type(List).__reduce__
    assert Callable.__reduce__() == "Callable"

    class Reducer:
        def __get__(self, obj, owner):
            return obj, owner

    parent = type(List)
    original = parent.__reduce__
    module = sys.modules["typing"]
    sys.modules["typing"] = ModuleType("replacement")
    try:
        assert type(Callable).__reduce__ is original and Callable.__reduce__() == "Callable"
        parent.__reduce__ = Reducer()
        assert type(Callable).__reduce__ == (None, type(Callable))
        assert Callable.__reduce__ == (Callable, type(Callable))
    finally:
        parent.__reduce__ = original
        sys.modules["typing"] = module

    assert type(namespace) is MappingProxyType
    assert namespace["__module__"] == TypeVar.__module__ == "typing"
    assert namespace["__doc__"] == TypeVar.__doc__ and TypeVar.__doc__.startswith("Type variable.")
    assert namespace["has_default"] is TypeVar.has_default
    assert namespace["has_default"](variable) is variable.has_default() is False
    substitute = namespace["__typing_subst__"]
    assert substitute is TypeVar.__typing_subst__
    assert substitute(variable, None) is type(None)
    assert variable.__typing_subst__(int) is int
    for unpacked in (Unpack[tuple[int, ...]], (*tuple[int, ...],)[0]):
        try:
            variable.__typing_subst__(unpacked)
        except TypeError as error:
            assert str(error) == f"{unpacked} is not valid as type argument"
        else:
            raise AssertionError("substituted an unpacked tuple for a type variable")
    try:
        namespace["extra"] = 1
    except TypeError:
        pass
    else:
        raise AssertionError("TypeVar namespace is writable")
    for name in ("__covariant__", "__contravariant__", "__infer_variance__"):
        member = getattr_static(variable, name)
        assert member is namespace[name] is getattr_static(TypeVar, name) is getattr(TypeVar, name)
        assert type(member).__name__ == "member_descriptor"
        assert member.__objclass__ is TypeVar and member.__name__ == name
        assert member.__get__(None, TypeVar) is member
        assert member.__get__(variable, TypeVar) is (name == "__covariant__")
        assert type(member).__get__(member, variable) is getattr(variable, name)
        for write in (lambda: setattr(variable, name, False),
                      lambda: delattr(variable, name)):
            try:
                write()
            except AttributeError as error:
                assert str(error) == "readonly attribute"
            else:
                raise AssertionError("variance flag was writable")

    class Other:
        flag = TypeVar.__covariant__

    assert Other.flag is TypeVar.__covariant__
    try:
        Other().flag = False
    except TypeError as error:
        print(error)
    else:
        raise AssertionError("member accepted unrelated receiver")

    from annotationlib import Format
    from typing import NoDefault, ParamSpec, ParamSpecArgs, ParamSpecKwargs, TypeAliasType, TypeVarTuple, cast

    for cls in (Any, TypeVar, TypeVarTuple, ParamSpec, ParamSpecArgs, ParamSpecKwargs, TypeAliasType):
        assert isinstance(cls, type) and issubclass(cls, object)
        assert cls.__base__ is object and cls.__bases__ == (object,)
        assert cls.__bases__ is cls.__bases__
        mro = cls.__mro__
        assert type(mro) is tuple and mro is cls.__mro__
        assert len(mro) == 2 and mro[0] is cls and mro[1] is object
    assert type(Any).__mro__ == (type(Any), type, object)
    assert not isinstance(Any, type("type", (), {}))
    for value in (variable, Callable, cast):
        assert not isinstance(value, type) and not hasattr(value, "__mro__")
    try:
        isinstance(1, Any)
    except TypeError:
        pass
    else:
        raise AssertionError("Any accepted an instance check")

    P = ParamSpec("P")
    parameters = (P, P.args, P.kwargs, TypeVarTuple("Ts"))
    for parameter in parameters:
        cls = type(parameter)
        members = vars(cls)
        assert type(members) is MappingProxyType
        assert members["__doc__"] == cls.__doc__ and cls.__doc__
        assert members["__module__"] == cls.__module__ == "typing"
    for cls, marker in ((ParamSpecArgs, P.args), (ParamSpecKwargs, P.kwargs)):
        origin = vars(cls)["__origin__"]
        assert origin is cls.__origin__ and origin.__objclass__ is cls
        assert origin.__get__(marker) is marker.__origin__ is P
        assert vars(cls)["__hash__"] is None
        compare = vars(cls)["__eq__"]
        assert type(compare).__name__ == "wrapper_descriptor"
        assert compare.__get__(marker)(cls(P))
    for parameter in (variable, P, parameters[-1]):
        cls = type(parameter)
        method = vars(cls)["has_default"]
        assert method is cls.has_default and method.__objclass__ is cls
        assert type(method).__name__ == "method_descriptor"
        assert method.__get__(parameter)() is parameter.has_default() is False
    assert isinstance(ParamSpec.__name__, str)
    assert vars(ParamSpec)["__name__"].__get__(P) == "P"

    sentinel_type = type(NoDefault)
    try:
        sentinel_type.new_attribute = 1
    except TypeError as error:
        assert str(error) == "cannot set 'new_attribute' attribute of immutable type 'NoDefaultType'"
    else:
        raise AssertionError("NoDefaultType accepted an attribute")
    assert not hasattr(sentinel_type, "new_attribute")

    assert type(NoDefault).__doc__ == "The type of the NoDefault singleton."
    assert NoDefault.__doc__ == type(NoDefault).__doc__
    assert NoDefault.__doc__ is NoDefault.__doc__

    marker = []
    for default in (None, (), marker, NoDefault):
        parameter = TypeVar("T", default=default)
        assert parameter.__default__ is default
        assert parameter.has_default() is (default is not NoDefault)
        assert parameter.evaluate_default(Format.VALUE) is default
    assert variable.evaluate_default(Format.VALUE) is NoDefault

    from typing import Generic

    required = TypeVar("U")
    defaulted = TypeVar("D", default=None)

    class Tail(Generic[variable, required, defaulted]): pass

    assert Tail[str, bool].__args__ == (str, bool, None)
    assert Tail[str, bool, None].__args__ == (str, bool, type(None))
    assert Tail[str, bool, int].__args__ == (str, bool, int)
    try:
        Tail[str]
    except TypeError as exc:
        assert str(exc).endswith("actual 1, expected at least 2")
    else:
        raise AssertionError("accepted a missing required type argument")
