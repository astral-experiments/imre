from re import Match
from patterns import PATTERNS


def find(text: str) -> Match[str] | None:
    return PATTERNS[0].search(text)
