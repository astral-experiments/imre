from pathlib import Path
from shutil import copyfile, copymode, copystat

try:
    copyfile(b"missing-\xff", "output.txt")
except FileNotFoundError as exc:
    print(repr(exc))
    print(repr(exc.errno))
    print(repr(exc.filename))
    print(repr(exc.filename2))

try:
    copyfile("file-link", "output.txt", follow_symlinks=False)
except FileExistsError as exc:
    print(str(exc))
    print(repr(exc.filename))
    print(repr(exc.filename2))

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
    copymode(b"input.txt", b"missing-\xfe/dst")
except FileNotFoundError as exc:
    print(str(exc))
    print(repr(exc.filename))
