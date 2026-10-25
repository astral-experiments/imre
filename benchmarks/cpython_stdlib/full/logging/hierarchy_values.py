"""Differential checks for changing parents, snapshots, and logger identity keys."""

from logging import getLogger
from pathlib import Path

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
levels = {ancestor: 1, middle: 2, child: 3}
levels[getLogger("Δ.deep.child")] = 4
print(len(levels))
print(levels[child])
pair = {(ancestor, child): 10}
print(pair[(getLogger("Δ"), child)])
paths = {Path("a/./b"): 1, Path("a/b"): 2}
print(len(paths))
print(paths[Path("a/b")])
