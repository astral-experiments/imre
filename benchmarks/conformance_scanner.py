"""Time the reporter's scanner on preloaded lines, or emit every capture for verification."""

from pathlib import Path
from sys import argv

from conformance_report.parse import scan_annotation


def main(path: str, rounds: int, dump: bool) -> None:
    lines = Path(path).read_text(encoding="utf-8").split("\0")
    matches = 0
    checksum = 0
    for _ in range(rounds):
        for number, line in enumerate(lines):
            found = scan_annotation(line)
            if found is None:
                continue
            matches += 1
            tag = found.tag or ""
            checksum += (
                1 + found.start + 3 * found.end + 5 * len(found.description)
                + 7 * len(tag) + 11 * int(found.optional) + 13 * int(found.multi)
            )
            if dump:
                print(f"{number}:{found.start}:{found.end}:{int(found.optional)}:{int(found.multi)}:{len(tag)}:{tag}{found.description}")
    print(f"{matches} {checksum}")


main(argv[1], int(argv[2]), argv[3] == "dump")
