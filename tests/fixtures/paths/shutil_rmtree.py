from pathlib import Path
from shutil import copyfile, rmtree

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
missing = b"missing-\xff"
try:
    copyfile(missing, "output.txt")
except FileNotFoundError as exc:
    print(repr(exc.filename))
    print(repr(exc.errno))
    print(repr(exc.filename2))
try:
    copyfile("file-link", "output.txt", follow_symlinks=False)
except FileExistsError as exc:
    print(repr(exc))
    print(repr(exc.filename))
    print(repr(exc.filename2))
dst = b"output.txt"
try:
    copyfile(b"file-link", dst, follow_symlinks=False)
except FileExistsError as exc:
    print(str(exc))
    print(repr(exc.filename))
    print(repr(exc.filename2))
