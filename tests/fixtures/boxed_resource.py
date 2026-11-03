import resource
from resource import RLIMIT_CORE, RLIMIT_NPROC, RLIMIT_STACK, RLIM_INFINITY, getrlimit, setrlimit


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
    print(resource.RLIMIT_NPROC, getrlimit(RLIMIT_NPROC))
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


def nofile_limits_child():
    import os
    from resource import RLIMIT_NOFILE

    assert RLIMIT_NOFILE == resource.RLIMIT_NOFILE
    saved = getrlimit(RLIMIT_NOFILE)
    soft = 64 if saved[0] == RLIM_INFINITY else max(0, saved[0] - 1)
    try:
        setrlimit(RLIMIT_NOFILE, (soft, saved[1]))
        assert getrlimit(RLIMIT_NOFILE) == (soft, saved[1])
        assert os.sysconf("SC_OPEN_MAX") == soft
    finally:
        setrlimit(RLIMIT_NOFILE, saved)
    assert getrlimit(RLIMIT_NOFILE) == saved


def core_limits_child():
    def check(expected):
        assert getrlimit(RLIMIT_CORE) == expected
        with open("/proc/self/limits") as limits:
            for line in limits.read().splitlines():
                if line.startswith("Max core file size "):
                    values = line.split()[4:6]
                    observed = tuple(RLIM_INFINITY if v == "unlimited" else int(v) for v in values)
                    assert observed == expected
                    return
        raise AssertionError("missing kernel core limit")

    assert setrlimit is resource.setrlimit
    assert setrlimit.__text_signature__ == "($module, resource, limits, /)"
    saved = getrlimit(RLIMIT_CORE)
    check(saved)
    soft = 1 if saved[0] == 0 and saved[1] != 0 else 0
    try:
        result = setrlimit(RLIMIT_CORE, iter([soft, saved[1]]))
        assert result is None
        check((soft, saved[1]))
    finally:
        setrlimit(RLIMIT_CORE, saved)
    check(saved)

    for call, expected in (
        (lambda: setrlimit(RLIMIT_CORE), TypeError),
        (lambda: setrlimit(resource=RLIMIT_CORE, limits=saved), TypeError),
        (lambda: setrlimit(-1, None), ValueError),
        (lambda: setrlimit(RLIMIT_CORE, 42), TypeError),
        (lambda: setrlimit(RLIMIT_CORE, (1,)), ValueError),
        (lambda: setrlimit(RLIMIT_CORE, (-2, 0)), ValueError),
        (lambda: setrlimit(RLIMIT_CORE, (1, 0)), ValueError),
        (lambda: setrlimit(RLIMIT_CORE, (2**100, 0)), OverflowError),
    ):
        try:
            call()
        except (TypeError, ValueError, OverflowError) as error:
            assert type(error) is expected
            print(type(error).__name__, str(error))
        else:
            raise AssertionError("invalid setrlimit call succeeded")
        check(saved)

    # The CLI harness runs this function in its own child process.
    setrlimit(RLIMIT_CORE, (0, 0))
    check((0, 0))
