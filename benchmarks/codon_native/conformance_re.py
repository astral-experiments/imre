"""Native RE2 adapter generated with the two equivalent annotation literals bound below."""

import re

VERBOSE = re.VERBOSE


def compile(pattern: str, flags: int):
    if flags != VERBOSE or pattern != ORIGINAL:
        raise ValueError("expected the pinned verbose Ruff annotation pattern")
    return re.compile(NATIVE)


def search(pattern, string: str):
    return pattern.search(string)
