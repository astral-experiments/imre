from builtins import __import__ as load
import sys

# Runtime module names allow this executable to run without bundling os.py.
posix = load(sys.argv[2])
names = ("chown", "fchown", "lchown", "getuid", "geteuid", "getgid", "getgroups")
getters = ("getegid",)
if sys.platform == "linux":
    getters += ("getresgid", "getresuid")
names += getters
for name in names:
    assert name in vars(posix)
os = load(sys.argv[1])
for name in names:
    assert vars(os)[name] is vars(posix)[name] is getattr(os, name)
for name in getters:
    getter = getattr(os, name)
    assert getter.__self__ is posix
    assert getter.__name__ == name
    print(getter.__text_signature__, getter.__doc__, getter())
    for args, kwargs in (((1,), {}), ((), {"value": 1})):
        try:
            getter(*args, **kwargs)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("credential getter accepted an argument")

print(os.geteuid())
