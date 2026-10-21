from pathlib import Path as Location


def child(root: Location, name: str) -> Location:
    return root / name
