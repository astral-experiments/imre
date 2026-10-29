import os
import posix


def current():
    read, write = os.pipe()
    try:
        assert isinstance(read, int) and read != write
        assert os.pipe is posix.pipe and os.close is posix.close
        assert not os.get_inheritable(read) and not os.get_inheritable(write)
        assert os.get_blocking(read) and os.get_blocking(write)
        assert os.set_inheritable(read, 1) is None
        assert os.set_blocking(read, []) is None
        assert os.get_inheritable(read) and not os.get_blocking(read)
        assert not os.get_inheritable(write) and os.get_blocking(write)
    finally:
        os.close(fd=read)
        posix.close(write)
    for operation in (os.close, os.get_inheritable, os.get_blocking):
        try:
            operation(read)
        except OSError as error:
            print(operation.__name__, error.errno)
        else:
            raise AssertionError("closed descriptor accepted")
