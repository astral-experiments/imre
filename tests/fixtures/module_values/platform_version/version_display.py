import platform
import sys


def matches_runtime() -> bool:
    return platform.python_version() == sys.version.split()[0]
