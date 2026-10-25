"""Differential checks for changing parents, snapshots, and logger identity keys."""

import logging
import logging.config
from logging import getLogger
from pathlib import Path

def target() -> logging.Logger:
    print("target")
    return getLogger("route.middle")


def value() -> bool:
    print("value")
    return False


root = getLogger()
child = getLogger("Δ.deep.child")
old = root.getChildren()
print(child.parent is root)
print(child in old)
ancestor = getLogger("Δ")
print(child.parent is ancestor)
print(child in ancestor.getChildren())
print(ancestor in old)
middle = getLogger("Δ.deep")
print(child.parent is middle)
print(child in middle.getChildren())
print(middle in ancestor.getChildren())
snapshot = root.getChildren()
snapshot.discard(ancestor)
print(ancestor in root.getChildren())
print(ancestor in snapshot)
early = getLogger("ro.early")
ro = getLogger("ro")
ro.setLevel(10)
late = getLogger("ro.late")
print(repr((early.parent is root, late.parent is ro)))
print(repr((early.getEffectiveLevel(), late.getEffectiveLevel())))
print(repr((early in ro.getChildren(), late in ro.getChildren())))
levels = {ancestor: 1, middle: 2, child: 3}
levels[getLogger("Δ.deep.child")] = 4
print(len(levels))
print(levels[child])
pair = {(ancestor, child): 10}
print(pair[(getLogger("Δ"), child)])
paths = {Path("a/./b"): 1, Path("a/b"): 2}
print(len(paths))
print(paths[Path("a/b")])

# Handler search ignores lastResort. An ancestor created after its child can
# stop the walk. Root's own propagation flag never hides an attached handler.
route = getLogger("route.middle.child")
print(route.hasHandlers())
route.warning("fallback")
logging.basicConfig(format="%(name)s:%(message)s")
print(route.hasHandlers())
root.propagate = False
print(root.hasHandlers())
print(route.hasHandlers())
target().propagate = value()
print(route.hasHandlers())
route.warning("stopped before root")
target().propagate = True
print(route.hasHandlers())
route.warning("reached root")
logging.config.dictConfig({"version": 1, "root": {"level": "WARNING"}})
print(root.hasHandlers())
print(route.hasHandlers())
