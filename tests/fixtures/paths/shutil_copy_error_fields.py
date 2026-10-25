from pathlib import Path
from shutil import copyfile, copymode, copystat, rmtree

missing = b"missing-\xff"
try:
    copyfile(missing, "output.txt")
except FileNotFoundError as exc:
    print(repr(exc))
    print(repr(exc.errno))
    print(repr(exc.filename))
    print(repr(exc.filename2))

destination = "output.txt"
try:
    copyfile("file-link", destination, follow_symlinks=False)
except FileExistsError as exc:
    print(str(exc))
    print(repr(exc.filename))
    print(repr(exc.filename2))
    filename2 = exc.filename2
    if isinstance(filename2, str):
        print(filename2 is destination)

try:
    copyfile(b"file-link", "output.txt", follow_symlinks=False)
except FileExistsError as exc:
    print(str(exc))
    print(repr(exc.filename))
    print(repr(exc.filename2))

try:
    copyfile("file-link", b"output.txt", follow_symlinks=False)
except FileExistsError as exc:
    print(str(exc))
    print(repr(exc.filename))
    print(repr(exc.filename2))

try:
    copystat(Path("missing-source"), "bad\0destination")
except FileNotFoundError as exc:
    print(str(exc))
    print(repr(exc.filename))

try:
    copyfile("input.txt", "missing directory/")
except FileNotFoundError as exc:
    print(repr(exc.filename))
    cause = exc.__cause__
    if isinstance(cause, IsADirectoryError):
        print(repr(cause.filename))

try:
    rmtree(b"missing-\xff")
except FileNotFoundError as exc:
    print(str(exc))
    print(repr(exc))
    print(repr(exc.errno))
    print(repr(exc.filename2))

try:
    copymode(b"input.txt", b"missing-\xfe/dst")
except FileNotFoundError as exc:
    print(str(exc))
    print(repr(exc.filename))
