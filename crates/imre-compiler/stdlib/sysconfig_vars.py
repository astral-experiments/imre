import _imp
import os
import sys

_BASE_PREFIX = os.path.normpath(sys.base_prefix)
_BASE_EXEC_PREFIX = os.path.normpath(sys.base_exec_prefix)
_PY_VERSION_SHORT = "%d.%d" % sys.version_info[:2]
_CONFIG_VARS = None


def get_config_vars(*args):
    global _CONFIG_VARS
    prefix = os.path.normpath(sys.prefix)
    exec_prefix = os.path.normpath(sys.exec_prefix)
    if (_CONFIG_VARS is None or _CONFIG_VARS['prefix'] != prefix
            or _CONFIG_VARS['exec_prefix'] != exec_prefix):
        # Report Imre's own installation and build flags.
        _CONFIG_VARS = {
            'prefix': prefix,
            'exec_prefix': exec_prefix,
            'base': prefix,
            'platbase': exec_prefix,
            'installed_base': _BASE_PREFIX,
            'installed_platbase': _BASE_EXEC_PREFIX,
            'py_version_short': _PY_VERSION_SHORT,
            'implementation': 'Imre',
            'implementation_lower': 'imre',
            'TZPATH': ('/usr/share/zoneinfo:/usr/lib/zoneinfo:/usr/share/lib/zoneinfo:/etc/zoneinfo'
                       if os.name == 'posix' else ''),
            'Py_GIL_DISABLED': 0,
            'Py_DEBUG': 0,
            'EXT_SUFFIX': _imp.extension_suffixes()[0],
        }
    if args:
        return [_CONFIG_VARS.get(name) for name in args]
    return _CONFIG_VARS


def get_config_var(name):
    return get_config_vars().get(name)
