from typing import Literal, get_args, get_origin


def current():
    ssl_mode = Literal["adhoc"]
    print(repr(ssl_mode), get_origin(ssl_mode) is Literal, get_args(ssl_mode))
    print(ssl_mode is Literal["".join(("ad", "hoc"))])
    print(Literal[1] == Literal[True], Literal[Literal[1], True, 1].__args__)
    print({Literal[1, 2]: "value"}[Literal[2, 1]])

    class Origin:
        marker = object()

        def __dir__(self):
            key = chr(0xdca2)
            return ["marker", "__hidden__", "marker", key, key]

    alias = type(ssl_mode)(Origin(), (1,))
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
