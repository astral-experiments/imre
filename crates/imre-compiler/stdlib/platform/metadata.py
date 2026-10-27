# Version and source-control accessors from CPython 3.14.5 Lib/platform.py.
_sys_version_cache = {}


def _sys_version(sys_version=None):
    if sys_version is None:
        sys_version = sys.version
    result = _sys_version_cache.get(sys_version)
    if result is None:
        name, version, build, date, compiler = _parse_version(sys_version, sys.platform)
        if hasattr(sys, "_git"):
            _, branch, revision = sys._git
        elif hasattr(sys, "_mercurial"):
            _, branch, revision = sys._mercurial
        else:
            branch = revision = ""
        result = name, version, branch, revision, build, date, compiler
        _sys_version_cache[sys_version] = result
    return result


def python_implementation():
    return _sys_version()[0]


def python_version():
    return _sys_version()[1]


def python_version_tuple():
    return tuple(_sys_version()[1].split("."))


def python_branch():
    return _sys_version()[2]


def python_revision():
    return _sys_version()[3]


def python_build():
    return _sys_version()[4:6]


def python_compiler():
    return _sys_version()[6]
