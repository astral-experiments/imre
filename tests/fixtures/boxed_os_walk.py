import os
import shutil
import tempfile


def current():
    root = tempfile.mkdtemp()
    try:
        child = os.path.join(root, "child")
        os.mkdir(child)
        with open(os.path.join(child, "file"), "wb") as stream:
            stream.write(b"x")
        assert list(os.walk(root, topdown=False)) == [
            (child, [], ["file"]),
            (root, ["child"], []),
        ]
        walk = os.walk(root)
        top, dirs, files = next(walk)
        assert (top, dirs, files) == (root, ["child"], [])
        dirs.clear()
        assert list(walk) == []
    finally:
        shutil.rmtree(root)
