import subprocess
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
