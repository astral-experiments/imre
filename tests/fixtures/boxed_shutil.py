import importlib
import os
import shutil
from shutil import copyfileobj


def current():
    assert importlib.import_module("shutil") is shutil
    assert copyfileobj is shutil.copyfileobj
    assert "tar" in dict(shutil.get_archive_formats())

    with open("source", "wb") as f:
        f.write(b"abc\x00def")
    try:
        with open("source", "rb") as src, open("destination", "wb") as dst:
            assert copyfileobj(src, dst, 2) is None
            assert src.tell() == dst.tell() == 7
        with open("destination", "rb") as dst:
            assert dst.read() == b"abc\x00def"
    finally:
        os.unlink("source")
        if os.path.exists("destination"):
            os.unlink("destination")
    print("shutil registry and file copy checked")
