from pathlib import Path


def ordered(paths: list[Path]) -> list[Path]:
    return sorted(paths)


print([str(path) for path in ordered([Path("a-b"), Path("a/z")])])
