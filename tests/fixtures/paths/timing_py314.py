from pathlib import Path


def create(directory: str) -> list[int]:
    Path(directory, "new.json").write_text("created", encoding="utf-8")
    return [1, 2]


def mark() -> int:
    Path("unexpected.txt").write_text("too late", encoding="utf-8")
    return 0


def reverse(directory: str) -> bool:
    create(directory)
    return False


# Python 3.14 snapshots iterdir at call; glob scans at first consumption.
eager = [path.name for path, value in zip(Path("eager").iterdir(), create("eager"))]
assert "|".join(eager) == "old.json"
lazy = sorted(path.name for path, value in zip(Path("lazy").glob("*.json"), create("lazy")))
assert "|".join(lazy) == "new.json|old.json"
values = sorted(Path("sorted-eager").iterdir(), key=lambda path: path.name, reverse=reverse("sorted-eager"))
assert len(values) == 1
values = sorted(Path("sorted-lazy").glob("*.json"), key=lambda path: path.name, reverse=reverse("sorted-lazy"))
assert len(values) == 2
try:
    for index, path in enumerate(Path("missing").iterdir(), mark()):
        pass
except FileNotFoundError:
    pass
assert not Path("unexpected.txt").exists()
failed = False
try:
    list(zip([], Path("missing").iterdir()))
except FileNotFoundError:
    failed = True
assert failed

# Both scans have already closed and snapshotted before the first body executes.
names: list[str] = []
for path in Path("mutate-eager").iterdir():
    Path("mutate-eager", "new.json").write_text("created", encoding="utf-8")
    names.append(path.name)
assert "|".join(names) == "old.json"
names = []
for path in Path("mutate-lazy").glob("*.json"):
    Path("mutate-lazy", "new.json").write_text("created", encoding="utf-8")
    names.append(path.name)
assert "|".join(names) == "old.json"
print("directory timing passed")
