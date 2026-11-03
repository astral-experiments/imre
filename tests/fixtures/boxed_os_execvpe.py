import errno
import os
import sys


def search_errors():
    os.mkdir("denied")
    os.mkdir("denied/notepad")
    os.mkdir("invalid")
    with open("invalid/notepad", "wb") as stream:
        stream.write(b"exit 99\n")
    os.chmod("invalid/notepad", 0o700)

    # Retain the first substantive error, or the last missing path component.
    for path, number, filename in (
        ("denied:invalid:missing", errno.EACCES, b"denied/notepad"),
        ("invalid:denied:missing", errno.ENOEXEC, b"invalid/notepad"),
        ("missing:found/notepad", errno.ENOTDIR, b"found/notepad/notepad"),
        ("found/notepad:missing", errno.ENOENT, b"missing/notepad"),
    ):
        try:
            os.execvpe("notepad", ["notepad"], {"PATH": path})
        except OSError as error:
            assert error.errno == number and error.filename == filename
        else:
            raise AssertionError("execvpe returned")

    os.environ["PATH"] = "found"
    for argv, env in (([], None), ([], {"PATH": "found"}), ([""], {"PATH": "found"})):
        try:
            os.execvpe("notepad", argv, env)
        except ValueError:
            pass
        else:
            raise AssertionError("execvpe accepted empty argv or argv[0]")


def check_bindings():
    calls = []

    def observe(*args):
        calls.append(args)
        raise RuntimeError("observed")

    os.execv = os.execve = observe
    os.get_exec_path = lambda env: ["rebound"]
    argv = ["arg"]
    for env in (None, {}):
        try:
            os.execvpe("name", argv, env)
        except RuntimeError as error:
            assert str(error) == "observed"
        expected = (b"rebound/name", argv)
        if env is not None:
            expected += (env,)
        assert calls.pop() == expected

    os._execvpe = lambda *args: calls.append(args)
    assert os.execvpe("new", argv, None) is None
    assert calls == [("new", argv, None)]


def replace(mode):
    os.environ["PATH"] = "missing:found" if mode == "inherited" else "missing"
    os.environ["IMRE_EXECVPE_PARENT"] = "inherited"
    file = "found/notepad" if mode == "slash" else "notepad"
    env = None if mode == "inherited" else {
        b"PATH": b"missing" if mode == "slash" else b"missing:found",
        "IMRE_EXECVPE_CHILD": "supplied",
    }
    script = ('test "$$" = "$1" && test "$0" = marker && '
              'test "${IMRE_EXECVPE_PARENT-unset}:${IMRE_EXECVPE_CHILD-unset}" = "$2" && '
              '{ echo replaced; exit 77; }')
    variables = "inherited:unset" if env is None else "unset:supplied"
    os.execvpe(file, ["sh", "-c", script, "marker", str(os.getpid()), variables], env)
    raise AssertionError("successful execvpe returned")


# Each interpreter gets its own directory under the harness's temporary cwd.
private = "execvpe-" + str(os.getpid())
os.mkdir(private, 0o700)
os.chdir(private)
os.mkdir("found")
os.symlink("/bin/sh", "found/notepad")
mode = sys.argv[1]
if mode == "errors":
    search_errors()
elif mode == "bindings":
    check_bindings()
else:
    replace(mode)
