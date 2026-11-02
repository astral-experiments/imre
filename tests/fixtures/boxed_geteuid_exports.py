from builtins import __import__ as load
import sys

# Runtime module names allow this executable to run without bundling os.py.
posix = load(sys.argv[2])
names = ("chown", "fchown", "lchown", "getuid", "geteuid", "getgid", "getgroups")
for name in names:
    assert name in vars(posix)
os = load(sys.argv[1])
for name in names:
    assert vars(os)[name] is vars(posix)[name] is getattr(os, name)
print(os.geteuid())
