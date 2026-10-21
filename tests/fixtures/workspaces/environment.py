from pathlib import Path
from tempfile import TemporaryDirectory


with TemporaryDirectory() as first:
    print(first[0] == "/")
    print(Path(first).parent.name)
    print(Path(first).name[:3] == "tmp")
    print(len(Path(first).name))
    print(Path(first).is_dir())
with TemporaryDirectory(dir=None, prefix="second-") as second:
    print(Path(first).parent == Path(second).parent)
    print(first != second)
print(Path(first).exists())
print(Path(second).exists())
cached = Path(first).parent
cached.replace("moved-default")
try:
    with TemporaryDirectory():
        print("unexpected fallback after caching")
except FileNotFoundError:
    print("default directory choice is cached")
finally:
    Path("moved-default").replace(cached)
