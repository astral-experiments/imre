import errno
import os
import posix


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


def current():
    assert os.waitpid is posix.waitpid
    assert os.WNOHANG == posix.WNOHANG
    # The current process is never its own child.
    try:
        os.waitpid(Index(os.getpid()), Index(os.WNOHANG))
    except ChildProcessError as error:
        assert error.errno == errno.ECHILD
    else:
        raise AssertionError("unexpected wait success")

    for args, kind in (((1 << 50, 0), OverflowError), ((-1, 1 << 50), OverflowError),
                       ((1.5, 0), TypeError), ((-1,), TypeError)):
        try:
            os.waitpid(*args)
        except kind:
            pass
        else:
            raise AssertionError("invalid waitpid arguments accepted")

    try:
        os.waitpid(-1, options=os.WNOHANG)
    except TypeError:
        pass
    else:
        raise AssertionError("options must be positional")
    return 42
