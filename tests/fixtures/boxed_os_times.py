import os
import posix
import sys
import time


def current():
    from os import times, times_result

    assert times is os.times is posix.times
    assert times_result is os.times_result is posix.times_result
    assert type(times_result) is type
    assert repr(times_result) == "<class 'posix.times_result'>"
    assert times_result.__module__ == "posix"
    assert times_result.__match_args__ == ("user", "system", "children_user", "children_system", "elapsed")
    sample = times()
    assert type(sample) is times_result
    assert isinstance(sample, tuple)
    assert tuple(sample) == (sample.user, sample.system, sample.children_user,
                             sample.children_system, sample.elapsed)
    assert all(type(item) is float for item in sample)
    if sys.platform == "linux":
        time.sleep(2 / os.sysconf("SC_CLK_TCK"))
        assert times().elapsed > sample.elapsed
    result = times_result(sequence=range(5), dict={})
    assert result == (0, 1, 2, 3, 4) and result.children_system == 3
    for args, kwargs in [((1,), {}), ((), {"user": 0})]:
        try:
            times(*args, **kwargs)
        except TypeError:
            pass
        else:
            raise AssertionError("times accepted an argument")
