import errno
from pathlib import Path
from tempfile import TemporaryDirectory


def pending_return(names: list[str]) -> list[Path]:
    with TemporaryDirectory(dir="scratch") as name:
        names.append(name)
        Path(name).rmdir()
        Path("source.txt").replace(name)
        return [Path(name), Path("sentinel.txt")]


names: list[str] = []
try:
    pending_return(names)
    print("return escaped")
except NotADirectoryError as error:
    print("cleanup replaces owned return")
    print((error.errno, error.strerror, repr(error.filename) == repr(names[0]), repr(error.args)))
Path(names[0]).replace("return-retained.txt")

body_name = ""
try:
    with TemporaryDirectory(dir="scratch") as body_name:
        Path(body_name).rmdir()
        Path("plain-file").replace(body_name)
        raise ValueError("body error")
except NotADirectoryError:
    print("cleanup replaces body error")
Path(body_name).replace("body-retained.txt")

outer = ""
try:
    with TemporaryDirectory(dir="scratch") as outer, TemporaryDirectory(dir=outer) as inner:
        Path(inner).rmdir()
        Path("destination.txt").replace(inner)
except NotADirectoryError:
    print("outer cleanup still runs")
print(Path(outer).exists())

first = ""
second = ""
try:
    with TemporaryDirectory(dir="scratch") as first, TemporaryDirectory(dir="scratch") as second:
        Path(first).rmdir()
        Path(second).rmdir()
        Path("body-retained.txt").replace(first)
        Path("root-symlink").replace(second)
except NotADirectoryError:
    print("outer cleanup error wins")
Path(first).replace("body-retained.txt")
Path(second).unlink()

cancelled = 0
for index in range(2):
    loop_name = ""
    try:
        with TemporaryDirectory(dir="scratch") as loop_name:
            Path(loop_name).rmdir()
            Path("return-retained.txt").replace(loop_name)
            break
    except NotADirectoryError:
        cancelled += 1
        Path(loop_name).replace("return-retained.txt")
print(cancelled)

# The harness limits descriptors. A traversal can exhaust them at any depth.
directories: list[Path] = []
nested = ""
try:
    with TemporaryDirectory(dir="scratch") as nested:
        directory = Path(nested)
        directories.append(directory)
        for _ in range(80):
            directory = directory / "d"
            directory.mkdir()
            directories.append(directory)
except OSError as error:
    assert error.errno == errno.EMFILE
    assert repr(error.filename).startswith(repr(nested)[:-1] + "/")
    assert repr(error.filename).endswith("/d'")
for index in range(len(directories) - 1, -1, -1):
    if directories[index].is_dir():
        directories[index].rmdir()
print(len(list(Path("scratch").iterdir())))
