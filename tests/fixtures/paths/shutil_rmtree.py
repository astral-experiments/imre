from pathlib import Path
from shutil import rmtree

print(rmtree.avoids_symlink_attacks)
rmtree(Path("nested"))
print(Path("nested").exists())
try:
    rmtree("dangling-link")
except FileNotFoundError as exc:
    print(repr(exc.errno))
    print(repr(exc.filename))
try:
    rmtree("directory-link")
except OSError as exc:
    print(str(exc))
    print(repr(exc))
    print(repr(exc.errno))
    print(repr(exc.strerror))
    print(repr(exc.filename))
    print(repr(exc.filename2))
print(Path("tree/a.json").exists())
try:
    rmtree("input.txt")
except NotADirectoryError as exc:
    print(str(exc))
    print(repr(exc.filename))
rmtree(b"input.txt", ignore_errors=True)
print(Path("input.txt").is_file())
