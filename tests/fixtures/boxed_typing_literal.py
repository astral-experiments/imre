import pickle
import typing
from typing import Literal, get_args, get_origin


def current():
    ssl_mode = Literal["adhoc"]
    print(repr(ssl_mode), get_origin(ssl_mode) is Literal, get_args(ssl_mode))
    print(ssl_mode is Literal["".join(("ad", "hoc"))])
    print(Literal[1] == Literal[True], Literal[Literal[1], True, 1].__args__)
    print({Literal[1, 2]: "value"}[Literal[2, 1]])

    for value in (Literal[typing.Any], Literal[1, 2], Literal[True, 2]):
        assert value.__name__ == value.__qualname__ == "Literal"
        assert value.__module__ == "typing"
        assert type(value).__reduce__ is typing._GenericAlias.__reduce__
        assert type(value).__reduce__(value) == value.__reduce__()
        for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
            assert pickle.loads(pickle.dumps(value, protocol)) is value
    assert Literal[1, 2] is not Literal[True, 2]

    class Origin:
        marker = object()

        def __dir__(self):
            key = chr(0xdca2)
            return ["marker", "__hidden__", "marker", key, key]

    alias = type(ssl_mode)(Origin(), (1,))
    assert alias.__module__ == Origin.__module__
    alias.__module__ = "alias module"
    assert alias.__module__ == "alias module" and Origin.__module__ != alias.__module__
    del alias.__module__
    assert alias.__module__ == "typing"
    names = dir(alias)
    fields = {"__args__", "__parameters__", "__origin__", "_inst", "_name"}
    methods = {"__format__", "__getstate__", "__init_subclass__", "__mro_entries__"}
    assert fields | methods | {"__class__"} <= set(names)
    assert all(callable(getattr(alias, name)) for name in methods)
    assert names.count("marker") == 1 and "__hidden__" not in names
    assert names.count(chr(0xdca2)) == 1
    assert alias.marker is Origin.marker and alias._name is None and alias._inst is True

    class InvalidOrigin:
        def __dir__(self):
            return [42]

    try:
        dir(type(alias)(InvalidOrigin(), (1,)))
    except AttributeError as error:
        assert "startswith" in str(error)
    else:
        raise AssertionError("invalid origin directory name")

    class Key:
        def __hash__(self):
            if broken:
                raise ValueError("changed hash")
            return 71

    broken = False
    arg = Key()
    alias = Literal[arg]
    broken = True
    for operation in (lambda: alias == alias, lambda: hash(alias)):
        try:
            operation()
        except ValueError as error:
            print(str(error))
