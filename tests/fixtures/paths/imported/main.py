import pathlib as fs
from facade import Location, child

root: fs.Path = fs.Path("tree")
value: Location = child(root, "a.json")
print(value.name)
print(value.read_text(encoding="utf-8"))
print(root / value.name)
