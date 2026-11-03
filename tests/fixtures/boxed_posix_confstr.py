import errno
import os
import posix


def outcome(name):
    try:
        return posix.confstr(name)
    except OSError as exc:
        return ("errno", exc.errno)


def current():
    query = os.confstr
    names = os.confstr_names
    assert query is posix.confstr and names is posix.confstr_names
    assert query.__self__ is posix
    print(query.__name__, query.__module__, query.__text_signature__, query.__doc__)
    path_code = names["CS_PATH"]
    path = query("CS_PATH")
    assert path and query(path_code) == path
    for name, code in names.items():
        value = outcome(name)
        assert value == outcome(code)
        print(name, code, repr(value))

    assert outcome(-1) == ("errno", errno.EINVAL)
    for args, kwargs, expected in (
        ((), {}, TypeError),
        ((path_code, path_code), {}, TypeError),
        ((), {"name": path_code}, TypeError),
        (("CS_invalid",), {}, ValueError),
        ((1.25,), {}, TypeError),
        ((1 << 60,), {}, OverflowError),
    ):
        try:
            query(*args, **kwargs)
        except (ValueError, TypeError, OverflowError) as exc:
            assert type(exc) is expected
            print(type(exc).__name__, str(exc))
        else:
            raise AssertionError((args, kwargs))

    class PathCode:
        def __index__(self):
            return path_code

    assert query(PathCode()) == path
    try:
        posix.confstr_names = {"alias": PathCode()}
        assert os.confstr_names is names and query("alias") == path
        del posix.confstr_names
        assert query(path_code) == path
        try:
            query("CS_PATH")
        except AttributeError:
            pass
        else:
            raise AssertionError("string lookup ignored the missing posix.confstr_names")
    finally:
        posix.confstr_names = names

    for name in ("confstr", "confstr_names"):
        value = vars(os)[name]
        delattr(os, name)
        try:
            assert not hasattr(os, name) and getattr(posix, name) is value
        finally:
            setattr(os, name, value)
