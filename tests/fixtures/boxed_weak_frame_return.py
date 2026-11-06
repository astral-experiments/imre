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
print("weak frame cleanup ok")
