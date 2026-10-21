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
except NotADirectoryError:
    print("cleanup replaces owned return")
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
print(len(list(Path("scratch").iterdir())))
