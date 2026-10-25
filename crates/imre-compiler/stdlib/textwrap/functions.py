"""Text wrapping functions compiled from CPython 3.14.5 Lib/textwrap.py (PSF-2.0)."""
from typing import Callable


def indent(text: str, prefix: str, predicate: Callable[[str], bool] | None = None) -> str:
    """Adds 'prefix' to the beginning of selected lines in 'text'.

    If 'predicate' is provided, 'prefix' will only be added to the lines
    where 'predicate(line)' is True. If 'predicate' is not provided,
    it will default to adding 'prefix' to all non-empty lines that do not
    consist solely of whitespace characters.
    """
    prefixed_lines: list[str] = []
    if predicate is None:
        # str.splitlines(keepends=True) doesn't produce the empty string,
        # so we need to use `str.isspace()` rather than a truth test.
        # Inlining the predicate leads to a ~30% performance improvement.
        for line in text.splitlines(True):
            if not line.isspace():
                prefixed_lines.append(prefix)
            prefixed_lines.append(line)
    else:
        for line in text.splitlines(True):
            if predicate(line):
                prefixed_lines.append(prefix)
            prefixed_lines.append(line)
    return ''.join(prefixed_lines)
