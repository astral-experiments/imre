import _imp
import os
import sys
from sysconfig import _INSTALL_SCHEMES, _getuserbase

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
            'userbase': _getuserbase(),
            'py_version_short': _PY_VERSION_SHORT,
            'implementation': 'Imre',
            'implementation_lower': 'imre',
            'Py_GIL_DISABLED': 0,
            'Py_DEBUG': 0,
            'TZPATH': ('/usr/share/zoneinfo:/usr/lib/zoneinfo:/usr/share/lib/zoneinfo:/etc/zoneinfo'
                       if os.name == 'posix' else ''),
            'EXE': sys._imre_executable_suffix,
            'EXT_SUFFIX': _imp.extension_suffixes()[0],
        }
    if args:
        return [_CONFIG_VARS.get(name) for name in args]
    return _CONFIG_VARS


def get_config_var(name):
    return get_config_vars().get(name)


def get_scheme_names():
    return tuple(sorted(_INSTALL_SCHEMES))


def get_preferred_scheme(key):
    return {'prefix': 'posix_prefix', 'user': 'posix_user'}[key]


def get_default_scheme():
    return get_preferred_scheme('prefix')


# Expansion follows CPython 3.14.5 Lib/sysconfig/__init__.py. See licenses/CPython.txt.
def _subst_vars(s, local_vars):
    try:
        return s.format(**local_vars)
    except KeyError as var:
        try:
            return s.format(**os.environ)
        except KeyError:
            raise AttributeError(f'{var}') from None


def get_paths(scheme=get_default_scheme(), vars=None, expand=True):
    if not expand:
        return _INSTALL_SCHEMES[scheme]
    if vars is None:
        fields = {key: value for key, value in get_config_vars().items() if isinstance(key, str)}
    else:
        keys = vars.keys()
        for key, value in get_config_vars().items():
            if key not in keys:
                vars[key] = value
        fields = {key: vars[key] for key in vars.keys() if isinstance(key, str)}
    return {name: os.path.normpath(_subst_vars(os.path.expanduser(template), fields))
            for name, template in _INSTALL_SCHEMES[scheme].items()}


def get_path(name, scheme=get_default_scheme(), vars=None, expand=True):
    return get_paths(scheme, vars, expand)[name]
