import atexit
import os
import posix
import sys


class Status:
    def __index__(self):
        os.write(1, b"converted\n")
        return 37


assert os._exit is posix._exit
for args, kwargs, error in (
    ((), {}, TypeError),
    ((1,), {"status": 2}, TypeError),
    ((), {"code": 1}, TypeError),
    ((1.5,), {}, TypeError),
    ((1 << 64,), {}, OverflowError),
):
    try:
        os._exit(*args, **kwargs)
    except error:
        pass
    else:
        raise AssertionError("invalid status was accepted")

atexit.register(os.write, 1, b"atexit\n")
sys.stdout.write("buffered")
try:
    os._exit(status=Status())
finally:
    os.write(1, b"finally\n")
