import errno
import os
import posix
import sys


def current():
    assert os.execv is posix.execv
    shell = b"/bin/sh"

    class Shell:
        def __fspath__(self):
            return shell

    events = []

    class Arg:
        def __fspath__(self):
            events.append("converted")
            return b"arg"

        def __del__(self):
            events.append("released")

    class Args(list):
        def __len__(self):
            return list.__len__(self)

        def __getitem__(self, index):
            events.append(list.__getitem__(self, index))
            return Arg()

    try:
        os.execv("/dev/null/exec", Args([0, 1]))
    except NotADirectoryError as error:
        assert error.errno == errno.ENOTDIR and error.filename is None
    else:
        raise AssertionError("invalid path was accepted")

    assert events == [0, "converted", "released", 1, "converted", "released"]
    read_fd, write_fd = os.pipe()
    try:
        assert os.write(write_fd, b"pipe-marker\n") == 12
        assert not os.get_inheritable(read_fd)
        assert not os.get_inheritable(write_fd)
        script = """
            test "$$" = "$1" && test "$0" = original &&
            test "$IMRE_EXECV_AUDIT" = changed &&
            test ! -e "/dev/fd/$3" &&
            read -r token < "/dev/fd/$2" && test "$token" = pipe-marker &&
            echo 42
        """
        argv = [b"original", "-c", script, b"original", str(os.getpid()),
                str(read_fd), str(write_fd)]
        veto = [True]

        def audit(event, args):
            if event == "os.exec":
                assert args[0] is shell and args[1] is argv and args[2] is None
                if veto:
                    raise RuntimeError("refused exec")
                argv[3] = b"mutated"
                os.putenv("IMRE_EXECV_AUDIT", "changed")
                os.set_inheritable(read_fd, True)

        sys.addaudithook(audit)
        try:
            os.execv(Shell(), argv)
        except RuntimeError as error:
            assert str(error) == "refused exec"
        else:
            raise AssertionError("vetoed exec returned")
        assert not os.get_inheritable(read_fd)
        veto.clear()
        os.execv(Shell(), argv)
        raise AssertionError("successful exec returned")
    finally:
        os.close(read_fd)
        os.close(write_fd)
