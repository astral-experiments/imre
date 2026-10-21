from pathlib import Path


class Source:
    root: Path

    def __init__(self, root: Path) -> None:
        self.root = root

    def iterdir(self, extra: str = "") -> list[Path]:
        print("ordinary iterdir" + extra)
        return [self.root / "a.json"]

    def glob(self, pattern: str) -> list[Path]:
        print("ordinary glob " + pattern)
        return [self.root / "b.json"]

    @staticmethod
    def values() -> list[Path]:
        return [Path("tree/a.json")]


def acquire(owners: list[Path]) -> Path:
    print("acquire")
    return owners.pop()


def maybe(path: Path, accepted: bool) -> Path | None:
    return path if accepted else None


def early() -> str:
    for outer in Path("nested").iterdir():
        for inner in outer.glob("*.json"):
            return inner.name
    return "empty"


source = Source(Path("tree"))
print("|".join(path.name for path in source.iterdir(extra=" argument")))
print("|".join(path.name for path in source.glob("arbitrary dynamic method")))
print("|".join(path.name for path in Source.values()))
print("|".join(path.name for path in source.values()))
owners = [Path("tree")]
print("|".join(sorted(path.name for path in acquire(owners).glob("[ab].json"))))
print(len(owners))
print("|".join(sorted(inner.name for outer in Path("nested").iterdir() for inner in outer.glob("*.json"))))
print(sum(1 for outer in Path("nested").glob("*") for inner in outer.iterdir()))
print(early())
selected = maybe(Path("tree"), True)
if isinstance(selected, Path):
    print(selected.name)
print(isinstance(Path("tree"), Path))
print(isinstance("tree", Path))
seen: list[str] = []
try:
    for path, value in zip(Path("tree").glob("[ab].json"), [1], strict=True):
        seen.append(path.name)
except ValueError:
    print("strict mismatch")
print(len(seen))
print(any(path.is_file() for path in Path("nested").glob("*") for path in path.iterdir()))
