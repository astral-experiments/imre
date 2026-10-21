from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable


@dataclass
class Locations:
    current: Path
    optional: Path | None = None
    history: list[Path] = field(default_factory=list)


def basename(path: Path) -> str:
    return path.name


def held(path: Path) -> Path:
    return path.parent / path.name


def consume(paths: list[Path]) -> str:
    return str(paths.pop())


path = Path("folder/./long filename beyond compact string capacity.txt")
record = Locations(path, path, [path])
copy = Locations(Path(str(path)), Path(str(path)), [Path(str(path))])
print(record == copy)
print(record)
print(repr(record.history))
print(repr((path, None, Path("../other"))))
print(f"{path}: {path!r}")
print(str(held(path)))
callback: Callable[[Path], str] = basename
print(callback(record.current))
values: dict[str, Path] = {"one": path, "two": Path("other")}
selected: Path | None = values.get("one")
if selected:
    print(selected.name)
record.current = values["two"]
record.history[0] = record.current
print(record != copy)
print(copy.current.name)
print(record.history[0].name)
print(record.optional is not None)
record.optional = None
print(record.optional is None)
paths = [Path("rooted long path beyond compact storage capacity")]
print(str(paths[0] / consume(paths)))
print(len(paths))
print(repr([Path("a"), Path("a"), Path("../b")]))
