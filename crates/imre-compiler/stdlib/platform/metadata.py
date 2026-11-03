# Runtime platform accessors from CPython 3.14.5 Lib/platform.py.
import os

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


def libc_ver(executable=None, lib='', version='', chunksize=16384):
    if not executable:
        try:
            parts = os.confstr('CS_GNU_LIBC_VERSION').split(maxsplit=1)
            if len(parts) == 2:
                return tuple(parts)
        except (AttributeError, ValueError, OSError):
            pass
        if not sys.executable:
            return lib, version
    raise NotImplementedError("scanning an executable for its libc version is unsupported")


def win32_is_iot():
    return win32_edition() in ('IoTUAP', 'NanoServer', 'WindowsCoreHeadless', 'IoTEdgeOS')

def win32_edition():
    try:
        import winreg
    except ImportError:
        pass
    else:
        try:
            cvkey = r'SOFTWARE\Microsoft\Windows NT\CurrentVersion'
            with winreg.OpenKeyEx(winreg.HKEY_LOCAL_MACHINE, cvkey) as key:
                return winreg.QueryValueEx(key, 'EditionId')[0]
        except OSError:
            pass

    return None


_WIN32_CLIENT_RELEASES = [
    ((10, 1, 0), "post11"),
    ((10, 0, 22000), "11"),
    ((6, 4, 0), "10"),
    ((6, 3, 0), "8.1"),
    ((6, 2, 0), "8"),
    ((6, 1, 0), "7"),
    ((6, 0, 0), "Vista"),
    ((5, 2, 3790), "XP64"),
    ((5, 2, 0), "XPMedia"),
    ((5, 1, 0), "XP"),
    ((5, 0, 0), "2000"),
]


def win32_ver(release='', version='', csd='', ptype=''):
    # On non-Windows hosts CPython's _win32_ver preserves the caller's
    # values and selects the client release table for any supplied version.
    if os.name == 'nt':
        raise NotImplementedError("Windows version detection is unsupported")
    if version:
        intversion = tuple(map(int, version.split('.')))
        release = next((r for v, r in _WIN32_CLIENT_RELEASES if v <= intversion), release)
    return release, version, csd, ptype


def _platform(*args):

    """ Helper to format the platform string in a filename
        compatible format e.g. "system-version-machine".
    """
    # Format the platform string
    platform = '-'.join(x.strip() for x in filter(len, args))

    # Cleanup some possible filename obstacles...
    platform = platform.replace(' ', '_')
    platform = platform.replace('/', '-')
    platform = platform.replace('\\', '-')
    platform = platform.replace(':', '-')
    platform = platform.replace(';', '-')
    platform = platform.replace('"', '-')
    platform = platform.replace('(', '-')
    platform = platform.replace(')', '-')

    # No need to report 'unknown' information...
    platform = platform.replace('unknown', '')

    # Fold '--'s and remove trailing '-'
    while True:
        cleaned = platform.replace('--', '-')
        if cleaned == platform:
            break
        platform = cleaned
    while platform and platform[-1] == '-':
        platform = platform[:-1]

    return platform


def _processor_from_subprocess():
    """
    Fall back to `uname -p`
    """
    try:
        import subprocess
    except ImportError:
        return None
    try:
        return subprocess.check_output(
            ['uname', '-p'],
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf8",
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        pass

_platform_cache = {}
_linux_host = None


def invalidate_caches():
    global _linux_host
    _linux_host = None
    _sys_version_cache.clear()
    _platform_cache.clear()


if sys.platform == "linux":
    def platform(aliased=False, terse=False):
        global _linux_host
        cached = _platform_cache.get((aliased, terse))
        if cached is not None:
            return cached
        if _linux_host is None:
            host = os.uname()
            processor = _processor_from_subprocess() or ''
            if processor in ('unknown', host.machine):
                processor = ''
            _linux_host = host, processor
        host, processor = _linux_host
        libcname, libcversion = libc_ver()
        result = _platform(host.sysname, host.release, host.machine, processor,
                           'with', libcname + libcversion)
        _platform_cache[(aliased, terse)] = result
        return result
