from builtins import __import__ as load
import sys

# Runtime names exercise the native module even when the program did not bundle os.py.
os = load(sys.argv[1])
posix = load(sys.argv[2])
for name in ("getxattr", "setxattr", "listxattr", "removexattr",
             "XATTR_CREATE", "XATTR_REPLACE", "XATTR_SIZE_MAX"):
    assert vars(os)[name] is vars(posix)[name]
    assert getattr(os, name) is getattr(posix, name)
