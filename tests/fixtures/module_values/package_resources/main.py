from pathlib import Path
import pkgutil

from parcel import describe
from parcel.reader import resource


def late_root() -> None:
    first = Path("first")
    other = Path("other")
    first.mkdir()
    other.mkdir()
    (first / "early.py").write_bytes(b"")
    for info in pkgutil.iter_modules([str(first), str(other)]):
        print(info.name)
        if info.name == "early":
            (other / "late.py").write_bytes(b"")
    (other / "late.py").unlink()
    (first / "early.py").unlink()
    other.rmdir()
    first.rmdir()


for entry in describe():
    print(entry)
for name in ["templates/greeting.bin", "templates/empty"]:
    data = resource(name)
    if data is not None:
        print(data == b"Hello\x00\xff\r\n")
try:
    resource("templates/missing")
except FileNotFoundError:
    print("missing resource")

try:
    pkgutil.get_data("parcel.reader.child", "unused")
except ModuleNotFoundError as error:
    print(str(error))
    assert isinstance(error.__cause__, AttributeError)
late_root()
