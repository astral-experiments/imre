"""Benchmark decode, comparison, and rendering after loading the input text once."""

from pathlib import Path
from sys import argv

from imre.json import decode

from .model import MemoryReport, ProjectComparison
from .render import render_summary


def preload(directory: Path) -> dict[str, str]:
    texts: dict[str, str] = {}
    for path in directory.glob("*.json"):
        texts[path.stem] = path.read_text(encoding="utf-8")
    return texts


def main() -> None:
    rounds = int(argv[1])
    if rounds < 0:
        raise ValueError("rounds must be nonnegative")
    old_texts = preload(Path(argv[2]))
    new_texts = preload(Path(argv[3]))
    checksum = 0
    rendered = ""
    for _ in range(rounds):
        old = {name: decode(text, MemoryReport) for name, text in old_texts.items()}
        new = {name: decode(text, MemoryReport) for name, text in new_texts.items()}
        names = sorted(name for name in old if name in new)
        projects = [ProjectComparison(name=name, old=old[name], new=new[name]) for name in names]
        rendered = render_summary(projects)
        checksum += len(rendered)
    print(checksum)
    print(rendered)


main()
