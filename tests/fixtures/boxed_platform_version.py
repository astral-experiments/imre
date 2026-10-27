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
