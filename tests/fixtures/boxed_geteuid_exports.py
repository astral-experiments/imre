import sys

# Runtime module names allow this executable to run without bundling os.py.
posix = __import__(sys.argv[2])
names = ("chown", "fchown", "lchown", "getuid", "geteuid", "getgid", "getgroups")
getters = ("getegid",)
if sys.platform == "linux":
    getters += ("getresgid", "getresuid")
names += getters + ("setregid",)
for name in names:
    assert name in vars(posix)
os = __import__(sys.argv[1])
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

setregid = os.setregid
assert setregid.__self__ is posix
assert setregid.__text_signature__ == "($module, rgid, egid, /)"
print(setregid.__doc__)

calls = []


class Group:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        calls.append(self.value)
        return self.value


# Both spellings of the no-change sentinel are accepted on our LP64 POSIX targets.
assert setregid(Group(-1), Group(2**32 - 1)) is None
assert calls == [-1, 2**32 - 1]
assert setregid(-1, os.getegid()) is None
for args, kwargs in (
    ((-2, Group(-1)), {}),
    ((-1, 2**32), {}),
    ((-1, 2**64), {}),
    ((None, -1), {}),
    ((-1,), {}),
    ((), {"rgid": -1, "egid": -1}),
):
    try:
        setregid(*args, **kwargs)
    except (OverflowError, TypeError) as exc:
        print(type(exc).__name__, str(exc))
    else:
        raise AssertionError("setregid accepted invalid arguments")
assert calls == [-1, 2**32 - 1]
