from fnmatch import fnmatchcase
from functools import wraps
from pathlib import Path
from typing import Callable


def original(name: str | bytes | Path, pattern: str | bytes | Path) -> bool:
    return True


def match(fn: Callable[[str | bytes | Path, str | bytes | Path], bool]) -> bool:
    return fn("name", "n*")


def exercise() -> None:
    @wraps(original, assigned=(), updated=())
    def decorated(name: str | bytes | Path, pattern: str | bytes | Path) -> bool:
        return False

    alias = decorated
    print(alias("name", "n*"), alias.__wrapped__ is original, match(fnmatchcase))


exercise()
