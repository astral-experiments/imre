from itertools import cycle, islice
from pathlib import Path, PurePosixPath, PureWindowsPath


def names(root: Path, pattern: str, follow: bool) -> str:
    return "|".join(sorted(str(p) for p in root.glob(pattern, recurse_symlinks=follow)))


tree = Path("tree")
for pattern in ["**", "**/", "**/*.json", "*/../*.JSON", "nested.json/**",
                "nested.json/**/", "nested.json/*/**", "**/*/**", "*/missing"]:
    print(pattern)
    print(names(tree, pattern, False))
print("|".join(sorted(str(p) for p in tree.rglob(Path("INSIDE.*"), case_sensitive=False))))

root = Path("empty-directory")
eager = root.iterdir()
lazy = root.glob("*")
alias = lazy
new = root / "new"
new.mkdir()
print(iter(alias) is lazy)
print(next(eager, None) is None)
print(next(lazy) == new)
print(next(alias, None) is None)
source = root.glob('new')
window = islice(source, 1)
print(next(window) == new)
print(next(window, None) is None)
print(next(source, None) is None)
repeated = cycle(root.iterdir())
print(next(repeated) == new)
print(next(repeated) == new)
print(set(islice(repeated, 3)) == {new})
print(len(set(source)) == 0)
print(set(root.glob('new')) == {new})
try:
    next(lazy)
except StopIteration:
    print("exhausted")
for bad in ["", ".", "./", "/absolute"]:
    try:
        tree.glob(bad)
    except ValueError as error:
        print(str(error))
    except NotImplementedError as error:
        print(str(error))

for part in ["", ".", "//", "/", "a", "A", ".."]:
    print(Path("//a/A/../a").parts.count(part))
    print(PurePosixPath("//a/A/../a").parts.count(part))
for part in ["", "C:\\", "c:\\", "Share", "share", ".."]:
    print(PureWindowsPath("C:/Share/share/../Share").parts.count(part))
