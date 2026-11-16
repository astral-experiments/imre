import errno
import os
import sys


def current():
    if sys.platform != "linux":
        return
    import termios

    assert not issubclass(termios.error, OSError)
    master, slave = os.openpty()
    try:
        original = termios.tcgetattr(slave)
        assert type(original) is list and len(original) == 7
        assert all(type(flag) is int for flag in original[:6])
        canonical = original.copy()
        canonical[3] |= termios.ICANON | termios.ECHO
        try:
            termios.tcsetattr(slave, termios.TCSANOW, canonical)
            with open(slave, "rb", buffering=0, closefd=False) as stream:
                attrs = termios.tcgetattr(stream)
                assert type(attrs[6]) is list and len(attrs[6]) == termios.NCCS
                assert all(type(cc) is bytes and len(cc) == 1 for cc in attrs[6])
                attrs[3] &= ~(termios.ICANON | termios.ECHO)
                attrs[6][termios.VMIN] = 2
                attrs[6][termios.VTIME] = 1
                assert termios.tcsetattr(stream, termios.TCSANOW, attrs) is None
            observed = termios.tcgetattr(slave)
            assert observed == attrs
            try:
                termios.tcsetattr(slave, termios.TCSANOW, original[:6] + [[]])
            except TypeError:
                assert termios.tcgetattr(slave) == observed
            else:
                raise AssertionError("tcsetattr accepted an empty control-character list")
        finally:
            termios.tcsetattr(slave, termios.TCSANOW, original)
        assert termios.tcgetattr(slave) == original

        with open(os.devnull, "rb") as file:
            for fn, args in (
                (termios.tcgetattr, (file,)),
                (termios.tcsetattr, (file, termios.TCSANOW, original)),
            ):
                try:
                    fn(*args)
                except termios.error as exc:
                    assert type(exc) is termios.error
                    assert exc.args == (errno.ENOTTY, os.strerror(errno.ENOTTY))
                else:
                    raise AssertionError("termios accepted a nonterminal descriptor")
    finally:
        os.close(slave)
        os.close(master)
