from builtins import __import__ as load
import sys

# Runtime module names allow this executable to run without bundling os.py.
os = load(sys.argv[1])
posix = load(sys.argv[2])
assert vars(os)["geteuid"] is vars(posix)["geteuid"] is os.geteuid
print(os.geteuid())
