import os
import shutil
import subprocess
import warnings


def current():
    cat = shutil.which("cat")
    true = shutil.which("true")
    assert cat and true
    reader, writer = os.pipe()
    try:
        # Conversion accepts int(), including decimal strings, after set deduplication.
        os.write(writer, b"kept")
        os.close(writer)
        writer = None
        output = subprocess.check_output([cat, "/dev/fd/" + str(reader)],
                                         pass_fds=iter([str(reader), str(reader)]))
        assert output == b"kept" and not os.get_inheritable(reader)

        for value, kind, message in (
            ([str(reader), reader], ValueError, "bad value(s) in fds_to_keep"),
            ((-1,), ValueError, "bad value(s) in fds_to_keep"),
            ((2 ** 70,), ValueError, "bad value(s) in fds_to_keep"),
            (([],), TypeError, "unhashable type"),
        ):
            try:
                subprocess.Popen([true], pass_fds=value)
            except kind as exc:
                assert message in str(exc)
            else:
                raise AssertionError("invalid pass_fds accepted")

        with warnings.catch_warnings():
            warnings.simplefilter("error", RuntimeWarning)
            try:
                subprocess.Popen([true], pass_fds=([],), close_fds=False, text=True,
                                 universal_newlines=False)
            except RuntimeWarning as exc:
                assert str(exc) == "pass_fds overriding close_fds."
            else:
                raise AssertionError("missing warning before argument conversion")

        for options, kind, expected in (
            ({"env": {"bad=name": "x"}}, ValueError, "illegal environment variable name"),
            ({"executable": "bad\x00"}, ValueError, "bad value(s) in fds_to_keep"),
            ({"env": {"A": "bad\x00"}}, ValueError, "bad value(s) in fds_to_keep"),
            ({"bufsize": 2 ** 47, "stdout": subprocess.PIPE}, OverflowError,
             "Python int too large to convert to C int"),
            ({"bufsize": 1, "stdout": subprocess.PIPE}, RuntimeWarning, "line buffering"),
        ):
            with warnings.catch_warnings():
                warnings.simplefilter("error", RuntimeWarning)
                try:
                    subprocess.Popen([true], pass_fds=(-1,), **options)
                except kind as exc:
                    assert expected in str(exc)
                else:
                    raise AssertionError("invalid descriptor launched child")

        # On the fork path a false-valued iterable can still yield a non-inheritable fd.
        class Keep:
            def __bool__(self):
                return False

            def __iter__(self):
                return iter((reader,))

        other = os.open(os.devnull, os.O_RDONLY)
        try:
            os.set_inheritable(other, True)
            with subprocess.Popen([cat, "/dev/fd/" + str(reader), "/dev/fd/" + str(other)],
                                  close_fds=False, pass_fds=Keep(), cwd=".") as child:
                assert child.wait() == 0
        finally:
            os.close(other)
        assert not os.get_inheritable(reader)
    finally:
        if writer is not None:
            os.close(writer)
        os.close(reader)
