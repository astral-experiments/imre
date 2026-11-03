import errno
import os
import subprocess
import sys
import warnings


class ZeroFlags:
    def __bool__(self):
        raise AssertionError("creationflags used as a boolean")

    def __ne__(self, other):
        assert other == 0
        return False


class Comparison:
    def __ne__(self, other):
        assert other == 0
        raise LookupError("compared")


def current():
    missing = ["/imre-missing-platform-option-control"]
    for launch, options, error, message in (
        (subprocess.call, {"group": -1}, ValueError, "Group ID cannot be negative, got -1"),
        (subprocess.Popen, {"group": []}, TypeError, "Group must be a string or an integer, not <class 'list'>"),
        (subprocess.Popen, {"group": 2 ** 64}, OverflowError, "gid is greater than maximum"),
        (subprocess.call, {"extra_groups": [-1]},
         ValueError, "Group ID cannot be negative, got -1"),
        (subprocess.Popen, {"extra_groups": [2 ** 64]}, ValueError, "invalid group id"),
        (subprocess.Popen, {"extra_groups": [2 ** 64], "executable": b"bad\x00"},
         ValueError, "embedded null byte"),
        (subprocess.Popen, {"extra_groups": [2 ** 64], "env": {"A": "bad\x00"}},
         ValueError, "embedded null byte"),
        (subprocess.call, {"startupinfo": False, "creationflags": Comparison()},
         ValueError, "startupinfo is only supported on Windows platforms"),
        (subprocess.check_call, {"creationflags": []},
         ValueError, "creationflags is only supported on Windows platforms"),
        (subprocess.check_output, {"creationflags": Comparison()}, LookupError, "compared"),
        (subprocess.Popen, {"startupinfo": 0, "pipesize": "bad"},
         TypeError, "pipesize must be an integer"),
        (subprocess.run, {"creationflags": 1, "text": True, "universal_newlines": False},
         ValueError, "creationflags is only supported on Windows platforms"),
    ):
        try:
            launch(missing, **options)
        except Exception as caught:
            assert type(caught) is error and str(caught) == message
        else:
            raise AssertionError("invalid launch accepted")

    with warnings.catch_warnings():
        warnings.simplefilter("error", RuntimeWarning)
        try:
            subprocess.call(missing, pass_fds=(1,), close_fds=False, startupinfo=1)
        except RuntimeWarning as caught:
            assert str(caught) == "pass_fds overriding close_fds."
        else:
            raise AssertionError("missing pass_fds warning")

    with subprocess.Popen(["/bin/sh", "-c", "printf ready; exit 47"], stdout=subprocess.PIPE,
                          startupinfo=None, creationflags=ZeroFlags()) as child:
        assert child.communicate() == (b"ready", None)
        assert child.returncode == 47 and child.stdout.closed

    gid = os.getgid()
    report_gid = [sys.executable, "-c", "import os; print(os.getgid())"]
    for requested in (gid, 65534 if gid != 65534 else 65533):
        for close_fds in (False, True):
            try:
                output = subprocess.check_output(report_gid, group=requested, close_fds=close_fds)
            except OSError as exc:
                # A user namespace can reject an unmapped group with EINVAL.
                assert requested != gid and exc.errno in (errno.EPERM, errno.EINVAL)
                assert exc.filename is None
            else:
                assert int(output) == requested
    assert os.getgid() == gid

    parent_groups = os.getgroups()
    report_groups = [sys.executable, "-c", "import os; print(os.getgroups())"]
    for groups in (None, [], [os.getegid()]):
        try:
            output = subprocess.check_output(report_groups, extra_groups=groups)
        except PermissionError as exc:
            assert groups is not None and exc.errno == errno.EPERM and exc.filename is None
        else:
            if groups is None or sys.platform == "linux":
                expected = parent_groups if groups is None else groups
                assert output == (str(expected) + "\n").encode()
        assert os.getgroups() == parent_groups
