import sys

# Runtime module names allow this executable to run without bundling os.py.
posix = __import__(sys.argv[2])
names = ("chown", "fchown", "lchown", "getuid", "geteuid", "getgid", "getgroups", "setgroups")
getters = ("getegid",)
if sys.platform == "linux":
    getters += ("getresgid", "getresuid")
names += getters + ("setreuid", "setregid")
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


class Identifier:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        converted.append(self.value)
        return self.value


for name, current, real, effective in (
    ("setreuid", os.geteuid(), "ruid", "euid"),
    ("setregid", os.getegid(), "rgid", "egid"),
):
    setter = getattr(os, name)
    assert setter.__self__ is posix and setter.__name__ == name
    assert setter.__text_signature__ == f"($module, {real}, {effective}, /)"
    print(setter.__doc__)

    # Both sentinel spellings leave process credentials alone on LP64 POSIX targets.
    converted = []
    assert setter(Identifier(-1), Identifier(2**32 - 1)) is None
    assert setter(-1, current) is None
    for args, kwargs in (
        ((-2, Identifier(-1)), {}),
        ((-1, 2**32), {}),
        ((-1, 2**64), {}),
        ((None, -1), {}),
        ((-1,), {}),
        ((), {real: -1, effective: -1}),
    ):
        try:
            setter(*args, **kwargs)
        except (OverflowError, TypeError) as exc:
            print(type(exc).__name__, str(exc))
        else:
            raise AssertionError(f"{name} accepted invalid arguments")
    assert converted == [-1, 2**32 - 1]


setter = os.setgroups
assert setter.__self__ is posix and setter.__text_signature__ == "($module, groups, /)"
groups = os.getgroups()
try:
    assert setter(tuple(groups)) is None
except PermissionError as exc:
    assert exc.errno == 1 and exc.filename is None
assert os.getgroups() == groups
converted = []
for value in (iter(()), {}, [None], (Identifier(-2),)):
    try:
        setter(value)
    except (OverflowError, TypeError) as exc:
        print(type(exc).__name__, str(exc))
    else:
        raise AssertionError("setgroups accepted invalid groups")
assert converted == [-2]
