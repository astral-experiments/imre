import os
import posix
from os import cpu_count

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
