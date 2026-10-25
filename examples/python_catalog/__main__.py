from gzip import open as gzip_open
from pathlib import Path
from sys import argv
from typing import TextIO

from imre.json import encode

from .catalog import CatalogEntry, Selection, finish_catalog, select_line


def read_catalog(stream: TextIO) -> dict[str, CatalogEntry]:
    selection = Selection({}, {})
    for physical in stream:
        # The upstream index reader also treats Unicode splitlines boundaries as separators.
        try:
            for line in physical.splitlines():
                select_line(selection, line)
        except Exception as selection_error:
            # The upstream bulk read surfaces input failures before a malformed record.
            for remaining in stream:
                pass
            raise selection_error
    return finish_catalog(selection)


def main() -> None:
    if len(argv) != 3:
        raise ValueError("expected INPUT.ndjson[.gz] OUTPUT.json")
    source = Path(argv[1])
    if source.name.endswith(".gz"):
        try:
            with gzip_open(str(source), "rt", encoding="utf-8") as stream:
                catalog = read_catalog(stream)
        except UnicodeDecodeError as text_error:
            # A bulk gzip text read validates the framing before decoding any bytes.
            if source.is_file():
                with gzip_open(str(source), "rb") as compressed:
                    while compressed.read(65536):
                        pass
            raise text_error
    else:
        with open(str(source), "r", encoding="utf-8") as stream:
            catalog = read_catalog(stream)
    rendered = encode(catalog, indent=2) + "\n"
    output = Path(argv[2])
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered, encoding="utf-8")


main()
