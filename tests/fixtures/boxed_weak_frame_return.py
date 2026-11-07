import gc
import weakref

events = []
roots = {}


class Handler:
    def emit(self):
        return 7


def invoke(method):
    assert method() == 7


def watch(target, label):
    def released(reference):
        assert reference() is None
        events.append(label)

    return weakref.ref(target, released)


def release_global():
    owner = roots.pop("handler")
    invoke(owner.emit)
    assert stable() is owner


def release_local():
    owner = Handler()
    reference = watch(owner, "local")
    invoke(owner.emit)
    assert reference() is owner
    return reference


class Temporary:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        events.append(self.label)


def observe(result, expected):
    assert not result
    assert events == expected


def tracked(label):
    owner = Handler()
    references.append(watch(owner, label))
    return owner


def fail():
    raise failure


roots["handler"] = Handler()
stable = watch(roots["handler"], "global")
for _ in range(64):
    invoke(roots["handler"].emit)
assert stable() is roots["handler"]
assert events == []
release_global()
assert stable() is None
assert events == ["global"]
local = release_local()
assert local() is None
assert events == ["global", "local"]
gc.collect()
assert events == ["global", "local"]
events.clear()
observe(Temporary("left") is Temporary("right"), ["right", "left"])
events.clear()
observe(Temporary("left") is Temporary("middle") is events.append("unexpected"), ["left", "middle"])
events.clear()
references = []
failure = ValueError("operand")
try:
    tracked("left") is not tracked("middle") is fail()
except ValueError as exc:
    assert exc is failure
else:
    raise AssertionError("lost operand exception")
assert events == ["left", "middle"]
print("weak frame cleanup ok")
