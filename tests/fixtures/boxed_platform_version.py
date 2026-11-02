import os
import platform
import sys


def current():
    original = sys.version
    assert platform.python_version() == original.split()[0]
    sys.version = "7.42 (branch, Dec 24 2024, 12:34:56) [custom compiler]"
    assert platform.python_version() == "7.42.0"
    assert platform.python_version_tuple() == ("7", "42", "0")
    sys.version = "3.15.0a1 free-threading build (branch) [compiler]"
    assert platform.python_version() == "3.15.0a1"
    for value in ("7.42", " 7.42 (branch) [cc]", "7.42 (branch) []", "7.42 (branch, bad-date) [cc]", "7.42.0 (Imre 1.2.3)"):
        sys.version = value
        try:
            platform.python_version()
        except ValueError as error:
            assert str(error) == "failed to parse CPython sys.version: " + repr(value)
        else:
            raise AssertionError(value)
    sys.version = original
    assert platform.python_version() == original.split()[0]

    confstr = getattr(os, "confstr", None)
    executable = sys.executable
    try:
        if confstr is not None:
            try:
                reported = confstr("CS_GNU_LIBC_VERSION")
            except (ValueError, OSError):
                reported = None
            if reported:
                assert platform.libc_ver() == tuple(reported.split(maxsplit=1))

        def alternate(name):
            assert name == "CS_GNU_LIBC_VERSION"
            return "other   1.2 build"

        os.confstr = alternate
        assert platform.libc_ver(lib="fallback", version="0") == ("other", "1.2 build")

        def unavailable(name):
            raise ValueError(name)

        os.confstr = unavailable
        sys.executable = ""
        assert platform.libc_ver(lib="fallback", version="0") == ("fallback", "0")
    finally:
        sys.executable = executable
        if confstr is None:
            del os.confstr
        else:
            os.confstr = confstr
