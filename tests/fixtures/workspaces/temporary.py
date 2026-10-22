from pathlib import Path
from tempfile import TemporaryDirectory


def directory(choice: int) -> Path | None:
    if choice == 0:
        return None
    return Path("scratch")


def text_directory(choice: int) -> str | None:
    if choice == 0:
        return None
    return "scratch"


with TemporaryDirectory(dir="scratch", prefix="work-λ-", suffix="-🙂") as name:
    original = name
    root = Path(name)
    print(name[0] == "/")
    print(root.parent.name)
    print(root.name[:len("work-λ-")] == "work-λ-")
    print(root.name[-len("-🙂"):] == "-🙂")
    print(len(root.name) == len("work-λ-") + 8 + len("-🙂"))
    print(root.is_dir())
    (root / "nested" / "deeper").mkdir(parents=True)
    (root / "nested" / "data.txt").write_text("temporary λ", encoding="utf-8")
    name = "rebound target"
    print((root / "nested" / "data.txt").read_text(encoding="utf-8"))
print(name)
print(Path(original).exists())
print(root.exists())

for choice in range(2):
    with TemporaryDirectory(dir=directory(choice)) as optional_path:
        print(Path(optional_path).parent.name)
    print(Path(optional_path).exists())
    with TemporaryDirectory(dir=text_directory(choice)) as optional_text:
        print(Path(optional_text).parent.name)
    print(Path(optional_text).exists())

with TemporaryDirectory(dir=Path("scratch"), prefix="", suffix="") as empty_parts:
    print(len(Path(empty_parts).name))
with TemporaryDirectory(dir="scratch", prefix="../relative-temp/name-") as traversed:
    print(Path(traversed).parent.name)
    print(traversed.replace("/../", "/") == traversed)
with TemporaryDirectory(dir="scratch") as outer:
    with TemporaryDirectory(dir="does-not-exist", prefix=outer + "/absolute-") as absolute:
        print(Path(absolute).parent == Path(outer))
    print(Path(absolute).exists())
print(Path(outer).exists())

with TemporaryDirectory(dir="scratch"):
    print(len(list(Path("scratch").iterdir())))
print(len(list(Path("scratch").iterdir())))
