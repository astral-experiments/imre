import os
import posix


def current():
    from os import ctermid, getloadavg, sync, uname, uname_result

    for function in (uname, getloadavg, ctermid, sync):
        name = function.__name__
        assert vars(os)[name] is vars(posix)[name] is function
        assert function.__self__ is posix and function.__text_signature__ == "($module, /)"
        for args, kwargs in (((1,), {}), ((), {"unexpected": 1})):
            try:
                function(*args, **kwargs)
            except TypeError as exc:
                print(str(exc))
            else:
                raise AssertionError("no-argument function accepted arguments")

    info = uname()
    assert type(info) is uname_result is posix.uname_result
    assert type(uname_result) is type and uname_result.__module__ == "posix"
    assert uname_result.__match_args__ == ("sysname", "nodename", "release", "version", "machine")
    assert (uname_result.n_fields, uname_result.n_sequence_fields, uname_result.n_unnamed_fields) == (5, 5, 0)
    assert tuple(info) == (info.sysname, info.nodename, info.release, info.version, info.machine)
    assert uname_result(tuple(info)) == info
    loads = getloadavg()
    assert type(loads) is tuple and len(loads) == 3
    assert all(type(load) is float for load in loads)
    assert type(ctermid()) is str
    assert sync() is None
