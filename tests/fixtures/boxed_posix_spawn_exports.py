from builtins import __import__ as load
import sys

# Runtime names exercise the native module even when the compiler did not bundle os.py.
posix = load(sys.argv[1])
if sys.argv[2] not in sys.modules:
    posix.posix_spawn = object()
    posix.POSIX_SPAWN_OPEN = 42
    if hasattr(posix, "POSIX_SPAWN_CLOSEFROM"):
        del posix.POSIX_SPAWN_CLOSEFROM

os = load(sys.argv[2])
for name in ("posix_spawn", "posix_spawnp", "POSIX_SPAWN_OPEN",
             "POSIX_SPAWN_CLOSE", "POSIX_SPAWN_DUP2"):
    assert vars(os)[name] is vars(posix)[name]
name = "POSIX_SPAWN_CLOSEFROM"
assert hasattr(os, name) == hasattr(posix, name)
if hasattr(posix, name):
    assert getattr(os, name) is getattr(posix, name)
