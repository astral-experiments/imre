import tomllib
from collections.abc import Sequence, Set as AbstractSet
from itertools import chain as concatenate
from pathlib import Path


def groups(path: Path) -> AbstractSet[str]:
    with open(path, "rb") as stream:
        return tomllib.load(stream).keys()


def candidates(names: AbstractSet[str], root: Path) -> Sequence[Path]:
    return [
        path
        for path in concatenate(root.glob("*.py"), root.glob("*.pyi"))
        if path.name.split("_")[0] in names
    ]


def paths(extras: Sequence[Path] = ()) -> str:
    return "|".join(map(str, extras))


def marked_root(path: Path, label: str) -> Path:
    print(label)
    return path


config = Path("groups.toml")
config.write_text(
    'z.two = 1\n"é" = 1985-04-12T23:20:50Z\nA = { value = [1,\n2] }\n[[n]]\n',
    encoding="utf-8",
)
names = groups(config)
print(len(names))
for name in names:
    print(name)
print("é" in names)
print("missing" not in names)
print(paths())
print(paths([config]))

folder = Path("chain-files")
folder.mkdir()
(folder / "z_main.py").write_text("", encoding="utf-8")
for item in concatenate(
    marked_root(folder, "first").glob("*.py"), marked_root(folder, "second").glob("*.pyi")
):
    print(item.name)
    if item.suffix == ".py":
        (folder / "A_next.pyi").write_text("", encoding="utf-8")
(folder / "other_helper.py").write_text("", encoding="utf-8")
for path in candidates(names, folder):
    print(path.name)

for invalid in [
    "one = 1\none = 2",
    "a = [1, ?]",
    'a = "\\x41"',
    '"\\e" = 1',
    "a = 12:34",
    "a = { b = 1,}",
    "a = {\nb = 1\n}",
    "a = 0000-01-01",
]:
    config.write_text(invalid, encoding="utf-8")
    try:
        groups(config)
    except tomllib.TOMLDecodeError:
        print("bad TOML")
with open(config, "wb") as binary:
    binary.write(b"a = '\xff'")
try:
    groups(config)
except UnicodeDecodeError:
    print("bad UTF-8")
