from pathlib import Path
from tempfile import TemporaryDirectory


class Holder:
    path: Path

    def __init__(self, path: Path) -> None:
        self.path = path

    def flag(self) -> bool:
        self.path = Path("rebound")
        return False

    def text(self, value: str) -> str:
        self.path = Path("rebound")
        return value


def text(label: str, value: str) -> str:
    print(label)
    return value


def number(label: str, value: int) -> int:
    print(label)
    return value


def flag(label: str, value: bool) -> bool:
    print(label)
    return value


def failed() -> str:
    print("argument fails")
    raise ValueError("argument")


Path(text("mkdir receiver", "ordered/child")).mkdir(
    exist_ok=flag("exist_ok", False), mode=number("mode", 0o700), parents=flag("parents", True)
)
Path(text("unlink receiver", "missing")).unlink(missing_ok=flag("missing_ok", True))
Path(text("replace receiver", "source.txt")).replace(target=text("replace target", "destination.txt"))
with TemporaryDirectory(
    suffix=text("suffix", "-tail"), dir=Path(text("dir", "scratch")), prefix=text("prefix", "ordered-")
) as name:
    print(Path(name).is_dir())
print(Path(name).exists())

try:
    with TemporaryDirectory(prefix=text("first", "first-"), suffix=failed(), dir=text("not evaluated", "scratch")):
        print("unreachable")
except ValueError:
    print("argument error")
print(len(list(Path("scratch").iterdir())))

try:
    Path(text("preserved receiver", "destination.txt")).replace(failed())
except ValueError:
    print("replace argument error")
print(Path("destination.txt").is_file())

holder = Holder(Path("borrowed-directory"))
holder.path.mkdir(exist_ok=holder.flag())
print(Path("borrowed-directory").is_dir())
print(holder.path)
Path("borrowed-source").write_text("borrowed receiver", encoding="utf-8")
holder.path = Path("borrowed-source")
print(holder.path.replace(holder.text("borrowed-target")))
print(Path("borrowed-target").read_text(encoding="utf-8"))
holder.path = Path("borrowed-target")
holder.path.unlink(holder.flag())
print(Path("borrowed-target").exists())
holder.path = Path("scratch")
with TemporaryDirectory(dir=holder.path, prefix=holder.text("borrowed-")) as borrowed:
    print(Path(borrowed).parent.name)
print(holder.path)
print(Path(borrowed).exists())
