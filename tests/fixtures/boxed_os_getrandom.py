import errno
import os
import posix
from os import getrandom


class Size:
    def __index__(self):
        return 17


def current():
    assert getrandom is os.getrandom and getrandom is posix.getrandom
    assert getrandom.__self__ is posix
    assert getrandom.__text_signature__ == "($module, /, size, flags=0)"
    assert os.GRND_NONBLOCK == posix.GRND_NONBLOCK
    assert os.GRND_RANDOM == posix.GRND_RANDOM
    assert getrandom(0) == b""
    data = getrandom(flags=0, size=Size())
    assert type(data) is bytes and len(data) == 17
    assert getrandom(0, os.GRND_NONBLOCK) == b""
    for args, kind, code in (
        ((0, -1), OSError, errno.EINVAL),
        ((-1,), OSError, errno.EINVAL),
        ((-1, 1.5), TypeError, None),
        ((1 << 100,), OverflowError, None),
        ((0, 1 << 32), OverflowError, None),
    ):
        try:
            getrandom(*args)
        except kind as error:
            if code is not None:
                assert error.errno == code
        else:
            raise AssertionError(args)
