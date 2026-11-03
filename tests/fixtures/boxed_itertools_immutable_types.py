import importlib
import itertools
import sys


def check_readonly(cls):
    for mutate, args in ((setattr, ("extra", 1)), (delattr, ("__next__",))):
        try:
            mutate(cls, *args)
        except TypeError as error:
            assert "immutable type 'itertools." + cls.__name__ + "'" in str(error)
        else:
            raise AssertionError(cls)


def current():
    retains = (itertools.chain, itertools.filterfalse, itertools._tee, itertools._grouper)

    class Mutable(itertools.chain):
        pass

    Mutable.extra = 3
    assert Mutable.extra == 3
    del Mutable.extra

    class Impostor:
        pass

    Impostor.__module__ = "itertools"
    Impostor.__name__ = "chain"
    itertools.chain = Impostor
    try:
        itertools.chain.extra = 7
        assert Impostor.extra == 7
        check_readonly(retains[0])
    finally:
        itertools.chain = retains[0]

    del sys.modules["itertools"]
    try:
        fresh = importlib.import_module("itertools")
        for cls in retains:
            check_readonly(cls)
            check_readonly(getattr(fresh, cls.__name__))
        assert list(fresh.chain([1], [2])) == [1, 2]
        assert list(retains[1](None, [0, 1])) == [0]
    finally:
        sys.modules["itertools"] = itertools
