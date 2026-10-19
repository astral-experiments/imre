from sys import argv


def arguments(flag: bool) -> list[str]:
    return argv if flag else []


def count(flag: bool) -> int:
    return len(argv) if flag else 0


def selected() -> str:
    return "imported branch"


def choose(flag: bool) -> str:
    return "unused" if flag else selected()
