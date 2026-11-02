import types
import typing

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
    assert type(descriptor) is types.GetSetDescriptorType
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
print("three read-only alias class namespaces")
