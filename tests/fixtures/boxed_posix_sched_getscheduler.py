import errno
import os
import posix
from os import sched_getscheduler


def current():
    get = sched_getscheduler
    assert get is posix.sched_getscheduler and get is os.sched_getscheduler
    assert get.__module__ == "posix" and get.__name__ == "sched_getscheduler"
    assert get.__text_signature__ == "($module, pid, /)"
    for name, value in (
        ("SCHED_OTHER", 0), ("SCHED_NORMAL", 0), ("SCHED_FIFO", 1), ("SCHED_RR", 2),
        ("SCHED_BATCH", 3), ("SCHED_IDLE", 5), ("SCHED_DEADLINE", 6),
        ("SCHED_RESET_ON_FORK", 0x40000000),
    ):
        assert posix.__dict__[name] == os.__dict__[name] == value

    class Pid:
        def __index__(self):
            return os.getpid()

    policy = get(0)
    assert type(policy) is int and policy == get(os.getpid()) == get(Pid())
    if os.path.exists("/proc/thread-self/stat"):
        with open("/proc/thread-self/stat") as stat:
            # policy is field 41. The final ')' terminates field 2, the command name.
            fields = stat.read().rsplit(")", 1)[1].split()
        # procfs omits the sched_reset_on_fork flag returned by the syscall.
        policy_mask = ~os.SCHED_RESET_ON_FORK
        assert (policy & policy_mask) == int(fields[38])
        parent = int(fields[1])
        if parent and os.path.exists("/proc/%d/stat" % parent):
            with open("/proc/%d/stat" % parent) as stat:
                assert (get(parent) & policy_mask) == int(stat.read().rsplit(")", 1)[1].split()[38])

    class BadPid:
        def __index__(self):
            raise failure

    failure = ValueError("from __index__")
    try:
        get(BadPid())
    except ValueError as exc:
        assert exc is failure
    else:
        raise AssertionError("lost __index__ exception")

    for args, kwargs, error in (
        ((), {}, TypeError), ((0, 0), {}, TypeError), ((), {"pid": 0}, TypeError),
        ((1.5,), {}, TypeError), ((1 << 31,), {}, OverflowError),
        ((-(1 << 31) - 1,), {}, OverflowError),
    ):
        try:
            get(*args, **kwargs)
        except error as exc:
            print(type(exc).__name__, str(exc))
        else:
            raise AssertionError("invalid scheduler query succeeded")

    for pid, number in ((-1, errno.EINVAL), (0x7fffffff, errno.ESRCH)):
        try:
            get(pid)
        except OSError as exc:
            assert exc.errno == number
            if number == errno.ESRCH:
                assert isinstance(exc, ProcessLookupError)
        else:
            raise AssertionError("kernel accepted invalid PID")
    class Policy:
        def __index__(self):
            return os.SCHED_RR

    for name in ("sched_get_priority_min", "sched_get_priority_max"):
        bound = getattr(os, name)
        assert bound is vars(posix)[name]
        assert bound.__text_signature__ == "($module, /, policy)"
        assert bound(policy=Policy()) == bound(os.SCHED_RR)
        assert bound(os.SCHED_OTHER) == 0
        for invalid, error in ((-23, OSError), (1 << 32, OverflowError), (None, TypeError)):
            try:
                bound(invalid)
            except error as exc:
                if isinstance(exc, OSError):
                    assert exc.errno == errno.EINVAL
            else:
                raise AssertionError("invalid scheduling policy accepted")
    assert os.sched_get_priority_min(os.SCHED_RR) <= os.sched_get_priority_max(os.SCHED_RR)
    assert os.sched_yield is posix.sched_yield and os.sched_yield() is None
    affinity = os.sched_getaffinity(0)
    assert type(affinity) is set and affinity == posix.sched_getaffinity(Pid())

    class Cpu:
        def __index__(self):
            return min(affinity)

    try:
        assert os.sched_setaffinity(0, iter([Cpu(), Cpu()])) is None
        assert os.sched_getaffinity(0) == {min(affinity)}
    finally:
        os.sched_setaffinity(0, affinity)
    assert os.sched_getaffinity(0) == affinity

    for mask, error in (([], OSError), ([-1], ValueError), ([1 << 31], OverflowError),
                        ([None], TypeError)):
        try:
            os.sched_setaffinity(0, mask)
        except error as exc:
            if isinstance(exc, OSError):
                assert exc.errno == errno.EINVAL
        else:
            raise AssertionError("invalid affinity mask accepted")
    assert type(os.sched_rr_get_interval(0)) is float
    try:
        os.sched_rr_get_interval(-1)
    except OSError as exc:
        assert exc.errno == errno.EINVAL
    else:
        raise AssertionError("interval query accepted an invalid PID")
    print("Linux scheduler policy query passed")
