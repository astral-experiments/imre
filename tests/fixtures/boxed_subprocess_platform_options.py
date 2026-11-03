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
    # A retained instance exposes the wait status even if child setup raised.
    retained = []

    class Retained(subprocess.Popen):
        def __init__(self, *args, **kwargs):
            retained.append(self)
            super().__init__(*args, **kwargs)

    uid, gid = os.getuid(), os.getgid()
    foreign_uid = 65534 if uid != 65534 else 65533
    foreign_gid = 65534 if gid != 65534 else 65533
    supplementary = {"extra_groups": [gid]}
    primary = {"group": foreign_gid}
    user = {"user": foreign_uid}
    errors = []
    for options in (supplementary, primary, user, {**primary, **user},
                    {**supplementary, **primary, **user}):
        try:
            process = Retained(["/bin/sh", "-c", "exit 23"], **options)
        except OSError as exc:
            assert exc.errno in (errno.EPERM, errno.EINVAL) and exc.filename is None
            errors.append(exc.errno)
            process = retained[-1]
            assert process.pid > 0 and process.returncode == 255
            assert process.wait() == 255
        else:
            errors.append(None)
            assert process.wait() == 23
        try:
            os.waitpid(process.pid, os.WNOHANG)
        except ChildProcessError as exc:
            assert exc.errno == errno.ECHILD
        else:
            raise AssertionError("child was not reaped")
    # Each combined call reports the first failing operation on this host.
    groups_error, group_error, user_error, pair_error, all_error = errors
    assert pair_error == (group_error or user_error)
    assert all_error == (groups_error or pair_error)

    missing = ["/imre-missing-platform-option-control"]
    for launch, options, error, message in (
        (subprocess.call, {"group": -1}, ValueError, "Group ID cannot be negative, got -1"),
        (subprocess.Popen, {"group": []}, TypeError,
         "Group must be a string or an integer, not <class 'list'>"),
        (subprocess.Popen, {"group": 2 ** 64}, OverflowError, "gid is greater than maximum"),
        (subprocess.call, {"group": -1, "extra_groups": [-2], "user": -3},
         ValueError, "Group ID cannot be negative, got -1"),
        (subprocess.call, {"user": -1}, ValueError, "User ID cannot be negative, got -1"),
        (subprocess.Popen, {"user": []}, TypeError, "User must be a string or an integer"),
        (subprocess.Popen, {"user": 2 ** 64}, OverflowError, "uid is greater than maximum"),
        (subprocess.call, {"extra_groups": [-1]}, ValueError, "Group ID cannot be negative, got -1"),
        (subprocess.call, {"extra_groups": [-1], "user": -2},
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

    for option, current_id, foreign_id, report in (
        ("group", gid, foreign_gid, "import os; print(os.getgid()); print(os.getegid())"),
        ("user", uid, foreign_uid, "import os; print(os.getuid()); print(os.geteuid())"),
    ):
        for requested in (current_id, foreign_id):
            for close_fds in (False, True):
                try:
                    output = subprocess.check_output([sys.executable, "-c", report],
                                                     close_fds=close_fds, **{option: requested})
                except OSError as exc:
                    assert requested != current_id
                    if exc.errno == errno.EACCES:
                        assert exc.filename == sys.executable
                    else:
                        # A user namespace can reject an unmapped ID with EINVAL.
                        assert exc.errno in (errno.EPERM, errno.EINVAL) and exc.filename is None
                else:
                    assert list(map(int, output.split())) == [requested, requested]
    assert (os.getuid(), os.getgid()) == (uid, gid)
