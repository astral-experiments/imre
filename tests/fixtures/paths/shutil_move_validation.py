import shutil
from pathlib import Path


def move_text(src: str, dst: str) -> None:
    try:
        shutil.move(src, dst)
    except ValueError as error:
        print(str(error))


def move_bytes(src: bytes, dst: bytes) -> None:
    try:
        shutil.move(src, dst)
    except ValueError as error:
        print(str(error))
    except TypeError as error:
        print(str(error))


move_text("bad\0", "empty-directory")
move_text("bad\0", "input.txt")
move_text("input.txt", "bad\0")
move_text("bad\0", "bad\0")
move_bytes(b"bad\0", b"empty-directory")
move_bytes(b"missing", b"empty-directory")
move_bytes(b"input.txt", b"bad\0")
try:
    shutil.move(Path("bad\0"), Path("empty-directory"))
except ValueError as error:
    print(str(error))
try:
    shutil.move("input.txt", b"empty-directory")
except TypeError as error:
    print(str(error))
