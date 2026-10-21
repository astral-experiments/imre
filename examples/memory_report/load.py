from relar.json import decode

from .model import MemoryReport


def load_report(path: str) -> MemoryReport:
    with open(path, "r", encoding="utf-8") as source:
        text = source.read()
    return decode(text, MemoryReport)
