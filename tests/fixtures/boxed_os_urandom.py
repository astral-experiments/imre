import os
import posix
from os import urandom


class Size:
    def __index__(self):
        return 17


def current():
    assert urandom is posix.urandom and urandom is os.urandom
    assert urandom.__self__ is posix and urandom.__text_signature__ == "($module, size, /)"
    assert urandom(0) == b""
    data = urandom(Size())
    assert type(data) is bytes and len(data) == 17 and data != urandom(17)
    for args, kwargs in (((), {}), ((1, 2), {}), ((), {"size": 2}), ((-1,), {}),
                         ((1 << 100,), {}), ((1.5,), {})):
        try:
            urandom(*args, **kwargs)
        except (TypeError, ValueError, OverflowError) as err:
            print(type(err).__name__, str(err))
        else:
            raise AssertionError("accepted an invalid size")
