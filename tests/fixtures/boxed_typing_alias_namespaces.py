import types
import typing


def current():
    import doctest

    alias = typing.List[int]
    assert alias.__origin__ is list and alias.__args__ == (int,)
    discovered = {case.id() for case in doctest.DocTestSuite(typing)}
    assert {"typing.get_args", "typing.get_origin"} <= discovered

    generic = typing._GenericAlias
    unpack = typing._UnpackGenericAlias
    alias_type = typing.TypeAliasType
    for cls in (generic, unpack, alias_type):
        namespace = vars(cls)
        assert type(namespace) is types.MappingProxyType
        assert namespace["__doc__"] == cls.__doc__
        assert "__init_subclass__" not in namespace
        try:
            namespace["extra"] = 1
        except TypeError:
            pass
        else:
            raise AssertionError("writable alias class dictionary")
    assert generic.__doc__ is unpack.__doc__ is None
    assert alias_type.__doc__.startswith("Type alias.")
    assert alias_type.__module__ == generic.__module__ == unpack.__module__ == "typing"
    assert generic.copy_with is vars(generic)["copy_with"]
    assert generic.__reduce__ is vars(generic)["__reduce__"]
    assert unpack.copy_with is generic.copy_with and "copy_with" not in vars(unpack)
    assert generic.copy_with(typing.List[int], (str,)) == typing.List[str]
    assert generic.__reduce__(typing.List[int])[1] == (typing.List, int)
    try:
        class Invalid(generic):
            pass
    except TypeError:
        pass
    else:
        raise AssertionError("lost the inherited _Final subclass hook")

    class Internal(generic, _root=True):
        pass


    alias = alias_type("Alias", int)
    assert vars(alias_type)["__name__"].__get__(alias) == "Alias"
    assert alias_type.__name__ == "TypeAliasType"
    for name in ("__value__", "__module__", "__parameters__", "__type_params__", "evaluate_value"):
        descriptor = vars(alias_type)[name]
        assert type(descriptor).__name__ == "getset_descriptor"
        assert descriptor.__objclass__ is alias_type and descriptor.__name__ == name
        assert descriptor.__get__(None, alias_type) is descriptor
        if name != "__module__":
            assert getattr(alias_type, name) is descriptor
        value = descriptor.__get__(alias)
        if name == "evaluate_value":
            assert value(1) is int
        else:
            assert value == getattr(alias, name)
        try:
            descriptor.__get__(object())
        except TypeError:
            pass
        else:
            raise AssertionError("alias field accepted unrelated receiver")
    reduce = vars(alias_type)["__reduce__"]
    assert type(reduce) is types.MethodDescriptorType
    assert reduce.__get__(alias)() == "Alias"

    Ts = typing.TypeVarTuple("Ts")
    pack = typing.Unpack[Ts]
    tuple_pack = typing.Unpack[tuple[int, str]]
    for name in ("__typing_is_unpacked_typevartuple__", "__typing_unpacked_tuple_args__"):
        prop = vars(unpack)[name]
        assert type(prop) is property and getattr(unpack, name) is prop
        assert name not in vars(generic)
        for value in (pack, tuple_pack):
            assert prop.__get__(value, unpack) == getattr(value, name)
    try:
        vars(unpack)["__typing_unpacked_tuple_args__"].__get__(typing.Unpack[list[int]], unpack)
    except TypeError as error:
        assert str(error) == "Unpack[...] must be used with a tuple type"
    else:
        raise AssertionError("Unpack accepted a non-tuple alias")
    class Origin(typing.Generic[typing.TypeVar("T")]):
        origin_only = 1
        _private = 2
        __origin_private__ = 3
        copy_with = "origin method"

    generic_alias = Origin[int]
    unpack_alias = typing.Unpack[typing.Tuple[int]]
    callable_alias = typing.Callable[..., int]
    for value in (generic_alias, unpack_alias, callable_alias):
        names = dir(value)
        assert names == sorted(set(value.__dir__()))
        assert {"__args__", "__origin__", "__parameters__", "_inst", "_name"} <= set(names)
        assert "_not_an_attribute" not in names
        assert "__unpacked__" not in names
    assert generic_alias.origin_only == 1 and generic_alias._private == 2
    assert {"origin_only", "_private"} <= set(dir(generic_alias))
    assert "__origin_private__" not in dir(generic_alias)
    assert "_getitem" in dir(unpack_alias) and unpack_alias._getitem is typing.Unpack._getitem
    assert unpack_alias._name is None and unpack_alias._inst is True
    assert callable_alias._name == "Callable" and callable_alias._inst is True
    assert type(callable_alias)(callable_alias.__origin__, callable_alias.__args__)._name is None
    assert "__module__" in dir(callable_alias) and "__module__" in dir(unpack_alias)
    assert callable_alias.__module__ == unpack_alias.__module__ == "typing"
    custom = type(callable_alias)(Origin, (int, str), inst=False)
    assert custom._name is None and custom._inst is False and custom.origin_only == 1
    assert custom.copy_with.__name__ == generic.copy_with.__name__ == "copy_with"
    assert custom.copy_with.__self__ is custom and custom.copy_with((str, int)).__args__ == (str, int)
    assert unpack_alias.copy_with((typing.Tuple[str],)) == typing.Unpack[typing.Tuple[str]]
    missing = object()
    module = getattr(custom, "__module__", missing)
    Origin.__module__ = "moved_origin"
    assert getattr(custom, "__module__", missing) is module
    if module is missing:
        assert "__module__" not in dir(custom)
    print("three read-only alias class namespaces")


if __name__ == "__main__":
    current()
