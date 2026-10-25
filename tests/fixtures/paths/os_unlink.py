import os
from pathlib import Path


def descriptor() -> int:
    print("descriptor")
    return -1


def filename() -> bytes:
    print("filename")
    return b"\xff-missing"


# Removing a symlink, including a dangling one, never removes its target.
os.remove(Path("file-link"))
print(Path("input.txt").exists())
os.unlink(b"dangling-link", dir_fd=None)
base_fd = os.open("tree", os.O_RDONLY)
try:
    os.unlink(path="a.json", dir_fd=base_fd)
    print(Path("tree/a.json").exists())
finally:
    os.close(base_fd)
try:
    os.unlink(dir_fd=descriptor(), path=filename())
except OSError as error:
    print(str(error))
try:
    os.remove(b"\xff-missing")
except FileNotFoundError as error:
    print(str(error))
try:
    os.unlink("input.txt/")
except NotADirectoryError as error:
    print(str(error))
print(Path("input.txt").exists())
try:
    os.remove("tree")
except IsADirectoryError as error:
    print(str(error))
try:
    os.remove("bad\0path", dir_fd=1267650600228229401496703205376)
except ValueError as error:
    print(str(error))
for descriptor_value in [1267650600228229401496703205376, -1267650600228229401496703205376]:
    try:
        os.unlink("input.txt", dir_fd=descriptor_value)
    except OverflowError as error:
        print(str(error))
