from pathlib import Path
from sys import argv

from relar.json import encode

from .catalog import generate_catalog


def main() -> None:
    if len(argv) != 3:
        raise ValueError("expected INPUT.ndjson OUTPUT.json")
    text = Path(argv[1]).read_text(encoding="utf-8")
    rendered = encode(generate_catalog(text), indent=2) + "\n"
    output = Path(argv[2])
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered, encoding="utf-8")


main()
