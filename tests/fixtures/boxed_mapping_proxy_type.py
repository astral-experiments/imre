from dataclasses import field
from types import MappingProxyType


def current():
    values = {'a': 1}
    view = MappingProxyType(mapping=values)
    assert type(view) is type(field(metadata=values).metadata) is MappingProxyType
    assert MappingProxyType.__module__ == 'builtins'
    assert MappingProxyType.__name__ == 'mappingproxy'
    assert MappingProxyType.__repr__ is not dict.__repr__
    assert MappingProxyType.__repr__(view) == view.__repr__() == "mappingproxy({'a': 1})"
    assert MappingProxyType.__str__(view) == view.__str__() == "{'a': 1}"
    class Display(str):
        pass

    display = Display('mapping display')

    class Mapping(dict):
        def __str__(self):
            return display

    assert MappingProxyType.__str__(MappingProxyType(Mapping())) is display
    values['b'] = 2
    assert list(view) == ['a', 'b'] and view['b'] == 2
    assert list(MappingProxyType(view)) == ['a', 'b']

    try:
        view['c'] = 3
    except TypeError:
        pass
    else:
        raise AssertionError("writable mapping proxy")

    for invalid in ([], (1,), 42):
        try:
            MappingProxyType(invalid)
        except TypeError:
            pass
        else:
            raise AssertionError("accepted a non-mapping")

    try:
        MappingProxyType.__repr__(values)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a dict as a proxy descriptor receiver")
    return 0
