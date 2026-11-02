import copyreg
import pickle
from typing import ParamSpec, ParamSpecArgs, ParamSpecKwargs

P = ParamSpec("P")


def current():
    local = ParamSpec("local")
    for cls, marker in ((ParamSpecArgs, P.args), (ParamSpecKwargs, P.kwargs)):
        for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
            restored = pickle.loads(pickle.dumps(marker, protocol))
            assert type(restored) is cls and restored.__origin__ is P
            assert restored == marker and restored is not marker

        reducer = copyreg.dispatch_table[cls]
        assert reducer(cls(42)) == (cls, (42,))
        assert pickle.loads(pickle.dumps(cls(42))) == cls(42)
        try:
            copyreg.pickle(cls, lambda value: (str, ("replacement",)))
            assert pickle.loads(pickle.dumps(marker)) == "replacement"
        finally:
            copyreg.pickle(cls, reducer)

        try:
            pickle.dumps(cls(local))
        except pickle.PicklingError:
            pass
        else:
            raise AssertionError("local ParamSpec was pickled")


if __name__ == "__main__":
    current()
