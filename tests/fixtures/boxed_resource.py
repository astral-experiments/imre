import resource
from resource import RLIMIT_STACK, RLIM_INFINITY, getrlimit


class Index:
    def __index__(self):
        return RLIMIT_STACK


class InvalidIndex:
    def __index__(self):
        return "stack"


def current():
    assert getrlimit is resource.getrlimit
    assert getrlimit.__doc__ is None
    assert getrlimit.__self__ is resource
    assert getrlimit.__module__ == "resource"
    assert getrlimit.__text_signature__ == "($module, resource, /)"
    limits = getrlimit(Index())
    assert type(limits) is tuple and len(limits) == 2
    assert limits == getrlimit(RLIMIT_STACK)
    assert getrlimit(True) == getrlimit(1)
    print(RLIMIT_STACK, RLIM_INFINITY, limits)
    for call in (
        lambda: getrlimit(),
        lambda: getrlimit(1, 2),
        lambda: getrlimit(resource=RLIMIT_STACK),
        lambda: getrlimit(-1),
        lambda: getrlimit(2**31 - 1),
        lambda: getrlimit(2**100),
        lambda: getrlimit(1.5),
        lambda: getrlimit(InvalidIndex()),
    ):
        try:
            call()
        except (TypeError, ValueError, OverflowError) as error:
            print(type(error).__name__, str(error))
        else:
            raise AssertionError("invalid resource call succeeded")
