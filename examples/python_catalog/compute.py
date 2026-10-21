"""Repeat the complete catalog transformation after one raw-text preload."""

from pathlib import Path
from sys import argv

from relar.json import encode

from .catalog import generate_catalog


def main() -> None:
    if len(argv) != 4:
        raise ValueError("expected ROUNDS INPUT.ndjson OUTPUT.json")
    rounds = int(argv[1])
    if rounds < 0:
        raise ValueError("rounds must be nonnegative")
    text = Path(argv[2]).read_text(encoding="utf-8")
    rendered = ""
    checksum = 0
    for _ in range(rounds):
        rendered = encode(generate_catalog(text), indent=2) + "\n"
        checksum += len(rendered)
    output = Path(argv[3])
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered, encoding="utf-8")
    print(checksum)


main()
