from pathlib import Path
from tempfile import TemporaryDirectory


def fail() -> str:
    raise ValueError("second argument")


with TemporaryDirectory(dir="scratch") as outer, TemporaryDirectory(dir=outer) as inner, open(Path(inner) / "data.txt", "w", encoding="utf-8") as stream:
    stream.write("mixed managers")
    print(Path(outer).is_dir())
    print(Path(inner).is_dir())
print(stream.closed)
print(Path(outer).exists())
print(Path(inner).exists())

with TemporaryDirectory(dir="scratch") as name, TemporaryDirectory(dir=name) as name:
    original_parent = Path(name).parent
    print(Path(name).is_dir())
print(original_parent.exists())
print(Path(name).exists())

earlier = ""
try:
    with TemporaryDirectory(dir="scratch") as earlier, TemporaryDirectory(dir="missing/parent") as later:
        print("unreachable")
except FileNotFoundError:
    print("second creation failed")
print(earlier != "")
print(Path(earlier).exists())

outside = open("managed-file.txt", "w", encoding="utf-8")
try:
    with outside, TemporaryDirectory(dir="missing"):
        print("unreachable")
except FileNotFoundError:
    print("file entered first")
print(outside.closed)

closed = open("closed-file.txt", "w", encoding="utf-8")
closed.close()
closed_name = ""
try:
    with TemporaryDirectory(dir="scratch") as closed_name, closed:
        print("unreachable")
except ValueError:
    print("second enter failed")
print(Path(closed_name).exists())

argument_name = ""
try:
    with TemporaryDirectory(dir="scratch") as argument_name, TemporaryDirectory(dir=fail()):
        print("unreachable")
except ValueError:
    print("second argument failed")
print(Path(argument_name).exists())
print(len(list(Path("scratch").iterdir())))
