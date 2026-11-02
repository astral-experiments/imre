from builtins import __import__ as load
import sys

# Load both names dynamically so the native program exercises its source-less os facade.
posix = load(sys.argv[1])
assert sys.argv[2] not in sys.modules
spawnp = posix.posix_spawnp
sentinel = object()
posix.posix_spawn = sentinel
posix.POSIX_SPAWN_OPEN = 42
if hasattr(posix, "POSIX_SPAWN_CLOSEFROM"):
    del posix.POSIX_SPAWN_CLOSEFROM

os = load(sys.argv[2])
assert os.posix_spawn is sentinel
assert os.posix_spawnp is spawnp
assert os.POSIX_SPAWN_OPEN == 42
assert not hasattr(os, "POSIX_SPAWN_CLOSEFROM")
