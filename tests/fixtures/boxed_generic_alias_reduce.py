import copyreg
import pickle
from collections.abc import Callable
from types import GenericAlias


def current():
    alias = tuple[int, str]
    assert alias.__reduce__() == (GenericAlias, (tuple, (int, str)))
    assert alias.__reduce_ex__(0) == alias.__reduce__()
    try:
        alias.__reduce_ex__(object())
    except TypeError:
        pass
    else:
        raise AssertionError("invalid pickle protocol accepted")
    cursor = iter(alias)
    assert cursor.__reduce__() == (iter, (alias,))
    starred = next(cursor)
    assert cursor.__reduce__() == (iter, ((),))

    factory, (fresh,) = starred.__reduce__()
    assert factory is next and type(fresh) is type(cursor)
    make_iter, (plain,) = fresh.__reduce__()
    assert make_iter is iter and type(plain) is GenericAlias
    assert plain == alias and plain.__args__ is starred.__args__
    assert next(fresh) == starred and next(fresh, None) is None

    callback = Callable[[int, str], bool]
    assert callback.__reduce__() == (type(callback), (Callable, ([int, str], bool)))
    assert GenericAlias.__reduce__(callback) == (type(callback), (Callable, callback.__args__))

    for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
        for value in (alias, starred, callback):
            restored = pickle.loads(pickle.dumps(value, protocol))
            assert type(restored) is type(value) and restored == value
        live, spent = pickle.loads(pickle.dumps((iter(alias), cursor), protocol))
        assert list(live) == [starred] and list(spent) == []

    copyreg.pickle(GenericAlias, lambda value: (str, ("registered",)))
    try:
        assert pickle.loads(pickle.dumps(alias)) == "registered"
        assert pickle.loads(pickle.dumps(callback)) == callback
    finally:
        del copyreg.dispatch_table[GenericAlias]
