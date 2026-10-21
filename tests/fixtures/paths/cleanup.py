from pathlib import Path


def fail(path: Path) -> str:
    raise ValueError(path.name)


def first() -> str:
    for path in Path("tree").iterdir():
        return "returned"
    return "empty"


for turn in range(200):
    for path in Path("tree").iterdir():
        break
    for path in Path("tree").glob("*.json"):
        break
    try:
        paths = [fail(path) for path in Path("tree").iterdir()]
    except ValueError:
        pass
    try:
        paths = [fail(path) for path in Path("tree").glob("*.json")]
    except ValueError:
        pass
    assert any(path for path in Path("tree").iterdir())
    assert not all(False for path in Path("tree").glob("*.json"))
    assert first() == "returned"
    try:
        Path("invalid.txt").read_text(encoding="utf-8")
    except UnicodeDecodeError:
        pass
    Path("recovered.txt").write_text("still openable", encoding="utf-8")
print(Path("recovered.txt").read_text(encoding="utf-8"))
