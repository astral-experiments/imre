"""CPython build-tree detection for the Linux object runtime."""
# CPython 3.14.5 Lib/sysconfig/__init__.py. See stdlib/docstrings/LICENSE.
import os
import sys
from os.path import realpath


def _safe_realpath(path):
    try:
        return realpath(path)
    except OSError:
        return path

if sys.executable:
    _PROJECT_BASE = os.path.dirname(_safe_realpath(sys.executable))
else:
    # sys.executable can be empty if argv[0] has been changed and Python is
    # unable to retrieve the real program name
    _PROJECT_BASE = _safe_realpath(os.getcwd())

# In a virtual environment, `sys._home` gives us the target directory
# `_PROJECT_BASE` for the executable that created it when the virtual
# python is an actual executable ('venv --copies' or Windows).
_sys_home = getattr(sys, '_home', None)
if _sys_home:
    _PROJECT_BASE = _sys_home

# set for cross builds
if "_PYTHON_PROJECT_BASE" in os.environ:
    _PROJECT_BASE = _safe_realpath(os.environ["_PYTHON_PROJECT_BASE"])

def is_python_build(check_home=None):
    if check_home is not None:
        import warnings
        warnings.warn(
            (
                'The check_home argument of sysconfig.is_python_build is '
                'deprecated and its value is ignored. '
                'It will be removed in Python 3.15.'
            ),
            DeprecationWarning,
            stacklevel=2,
        )
    for fn in ("Setup", "Setup.local"):
        if os.path.isfile(os.path.join(_PROJECT_BASE, "Modules", fn)):
            return True
    return False

_PYTHON_BUILD = is_python_build()
