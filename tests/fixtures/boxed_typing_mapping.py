from collections import defaultdict
from collections.abc import Mapping as MappingABC, MutableMapping as MutableMappingABC, Sequence as SequenceABC
from typing import DefaultDict, Dict, Generic, Mapping, MutableMapping, Sequence, TypeVar, get_args, get_origin


def current():
    assert get_origin(Dict) is dict
    assert get_origin(DefaultDict) is defaultdict
    assert DefaultDict.__doc__ == "Deprecated alias to collections.defaultdict."
    assert get_origin(Sequence) is get_origin(Sequence[int]) is SequenceABC
    assert issubclass(tuple, Sequence) and not issubclass(dict, Sequence)
    assert get_origin(Mapping) is MappingABC
    assert issubclass(MutableMapping, Mapping) and not issubclass(Mapping, MutableMapping)
    assert get_origin(MutableMapping) is MutableMappingABC
    assert MutableMapping.__doc__ == "Deprecated alias to collections.abc.MutableMapping."
    assert issubclass(Dict, MutableMapping) and isinstance({}, MutableMapping)
    class Registered:
        pass
    assert not issubclass(Registered, MutableMapping)
    MutableMappingABC.register(Registered)
    assert issubclass(Registered, MutableMapping) and not issubclass(Registered, Dict)
    key = TypeVar("key")
    for alias in (Dict, DefaultDict, Mapping, MutableMapping):
        typed = alias[str, int]
        assert typed is alias[str, int] and get_args(typed) == (str, int)
        assert get_origin(typed) is get_origin(alias)
        assert alias[key, int][str] == typed
        try:
            alias[str]
        except TypeError as error:
            assert str(error) == f"Too few arguments for {alias}; actual 1, expected 2"
        else:
            raise AssertionError("mapping accepted a single type")
        try:
            isinstance({}, typed)
        except TypeError as error:
            assert str(error) == "Subscripted generics cannot be used with class and instance checks"
        else:
            raise AssertionError("parameterized mapping accepted a class check")

    class Entries(Dict[str, int]):
        pass
    assert Entries.__bases__ == (dict, Generic)
    for constructor in (DefaultDict, DefaultDict[str, int]):
        instance = constructor(int, present=3)
        assert type(instance) is defaultdict and instance.default_factory is int
        assert instance["present"] == 3 and instance["missing"] == 0
    for constructor in (Dict, Dict[str, int]):
        try:
            constructor()
        except TypeError as error:
            assert str(error) == "Type Dict cannot be instantiated; use dict() instead"
        else:
            raise AssertionError("Dict constructed an instance")
    for constructor in (MutableMapping, MutableMapping[str, int]):
        try:
            constructor()
        except TypeError as error:
            assert str(error).startswith("Can't instantiate abstract class MutableMapping")
        else:
            raise AssertionError("abstract mapping constructed an instance")
    print("mapping aliases")
