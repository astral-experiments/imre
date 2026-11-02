import sys


def current():
    info = vars(sys)["thread_info"]
    assert info is sys.thread_info
    owner = type(info)
    assert info.__class__ is owner and isinstance(info, owner)
    assert isinstance(info, tuple) and issubclass(owner, tuple)
    assert (owner.__module__, owner.__name__) == ("sys", "thread_info")
    assert (info.n_fields, info.n_sequence_fields, info.n_unnamed_fields) == (3, 3, 0)
    assert owner.__match_args__ == ("name", "lock", "version")
    assert tuple(info) == (info.name, info.lock, info.version)
    assert owner(tuple(info)) == info
    if sys.platform.startswith(("linux", "android", "freebsd")):
        assert info.name == "pthread"
    elif sys.platform == "win32":
        assert info.name == "nt"
    if sys.implementation.name == "imre":
        assert info.lock == ("mutex+cond" if info.name == "pthread" else None)
    else:
        assert info.lock in ("semaphore", "mutex+cond", None)
    assert info.version is None or isinstance(info.version, str)
    try:
        info.name = "replacement"
    except AttributeError:
        pass
    else:
        raise AssertionError("mutable thread_info")
    print("boxed sys.thread_info")


current()
