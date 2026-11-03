import os
import posix
from os import cpu_count, process_cpu_count

assert cpu_count is os.cpu_count is posix.cpu_count
assert vars(os)["cpu_count"] is vars(posix)["cpu_count"] is cpu_count
print(cpu_count(), cpu_count.__module__, cpu_count.__qualname__, cpu_count.__self__ is posix)
print(cpu_count.__doc__, cpu_count.__text_signature__)
for args, kwargs in (((1,), {}), ((), {"x": 1}), ((1,), {"x": 1})):
    try:
        cpu_count(*args, **kwargs)
    except TypeError as error:
        print(str(error))
    else:
        raise AssertionError("cpu_count accepted arguments")
os.environ["PYTHON_CPU_COUNT"] = "333"
print(cpu_count())

assert vars(os)["process_cpu_count"] is process_cpu_count
assert not hasattr(posix, "process_cpu_count")
overridden = process_cpu_count is cpu_count
print(overridden)
system_count = cpu_count()
affinity = os.sched_getaffinity(0)
assert process_cpu_count() == (system_count if overridden else len(affinity))
if len(affinity) > 1:
    try:
        os.sched_setaffinity(0, {min(affinity)})
    except PermissionError:
        pass
    else:
        try:
            assert process_cpu_count() == (system_count if overridden else 1)
            assert cpu_count() == system_count
        finally:
            os.sched_setaffinity(0, affinity)
