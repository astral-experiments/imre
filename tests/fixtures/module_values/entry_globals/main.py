"""An entry module with ordered initializers."""

from __future__ import annotations

import os
from pathlib import Path


def initialize() -> list[str]:
    print("initialize")
    return [__doc__, os.environ.get("IMRE_UNSET_ENTRY_FIXTURE", "fallback")]


VALUES = initialize()
BASE = Path("a") / ".." / "b"


def report() -> None:
    VALUES.append("from main")
    print("|".join(VALUES))
    print(str(BASE.absolute()) == str(Path(".").resolve()) + "/a/../b")


if __name__ == "__main__":
    report()
