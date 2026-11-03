import errno
import posix
import sys
from builtins import __import__ as load


def check_exports(os_name):
    original = posix.execve
    replacement = object()
    posix.execve = replacement
    sys.modules.pop(os_name, None)
    os = load(os_name)
    assert vars(os)["execve"] is replacement
    assert replacement in os.supports_fd and original not in os.supports_fd
    del os.execve
    assert not hasattr(os, "execve") and replacement in os.supports_fd

    del posix.execve

    def fallback(name):
        if name == "execve":
            return replacement
        raise AttributeError(name)

    posix.__getattr__ = fallback
    sys.modules.pop(os_name)
    os = load(os_name)
    assert not hasattr(os, "execve")
    assert replacement not in os.supports_fd and original not in os.supports_fd


def replace(os, mode):
    assert os.execve is posix.execve and os.execve in os.supports_fd
    assert os.execv not in os.supports_fd
    shell = b"/bin/sh"
    fd = os.open(shell, os.O_RDONLY)
    os.close(fd)
    for path, number in ((b"/dev/null/program", errno.ENOTDIR), (fd, errno.EBADF)):
        try:
            os.execve(path, ["program"], {})
        except OSError as error:
            assert error.errno == number and error.filename == path
        else:
            raise AssertionError("execve returned")

    class Descriptor:
        def __index__(self):
            return fd

        def __fspath__(self):
            raise AssertionError("descriptor should take precedence")

    if mode == "fd":
        fd = os.open(shell, os.O_RDONLY)
        target = Descriptor()
    else:
        target = shell
    for argv, env, error in (
        (["sh"], {"a\0b": "x"}, ValueError),
        (["sh"], {"a": b"x\0y"}, ValueError),
        (["sh"], {b"a=b": "x"}, ValueError),
        ((), {}, ValueError),
        ([""], {}, ValueError),
        ("sh", {}, TypeError),
        (["sh"], None, TypeError),
    ):
        try:
            os.execve(target, argv, env)
        except error:
            pass
        else:
            raise AssertionError("execve accepted invalid arguments")

    posix.putenv("IMRE_EXECVE_PARENT_ONLY", "parent")
    script = ('test "$$" = "$1" && test "$0" = marker && test "$TOKEN" = supplied && '
              'test -z "${IMRE_EXECVE_PARENT_ONLY+x}" && { echo replaced; exit 17; }')
    args = (b"sh", "-c", script, "marker", str(os.getpid()))
    os.execve(env={b"TOKEN": b"supplied", b"=LEADING": b"allowed"}, argv=args, path=target)
    raise AssertionError("successful execve returned")


mode, os_name = sys.argv[1:]
if mode == "exports":
    check_exports(os_name)
else:
    replace(load(os_name), mode)
