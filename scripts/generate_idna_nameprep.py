"""Generate the Unicode 3.2 Nameprep differences used by CPython 3.14.5."""

from itertools import groupby
from pathlib import Path
import stringprep
import sys
import unicodedata


PROHIBITED = (
    stringprep.in_table_c12, stringprep.in_table_c22, stringprep.in_table_c3,
    stringprep.in_table_c4, stringprep.in_table_c5, stringprep.in_table_c6,
    stringprep.in_table_c7, stringprep.in_table_c8, stringprep.in_table_c9,
)


def classification(point):
    char = chr(point)
    if any(check(char) for check in PROHIBITED):
        return "P"
    if stringprep.in_table_d1(char):
        return "R"
    if stringprep.in_table_d2(char):
        return "L"
    return "-"


def generate(output):
    for kind, group in groupby(range(sys.maxunicode + 1), key=classification):
        if kind != "-":
            start = next(group)
            end = max(group, default=start)
            print(f"{kind} {start:X} {end:X}", file=output)

    for point in range(sys.maxunicode + 1):
        char = chr(point)
        mapped = "" if stringprep.in_table_b1(char) else stringprep.map_table_b2(char)
        decomposed = unicodedata.ucd_3_2_0.normalize("NFKD", mapped)
        # Ordinary cases use Imre's Unicode 16 lowercase and decomposition.
        if decomposed != unicodedata.normalize("NFKD", char.lower()):
            print("M", f"{point:X}", *(f"{ord(c):X}" for c in decomposed), file=output)


if __name__ == "__main__":
    if sys.version_info[:3] != (3, 14, 5) or unicodedata.unidata_version != "16.0.0":
        raise SystemExit("Generate with stock CPython 3.14.5")
    root = Path(__file__).resolve().parents[1]
    table = root / "crates/imre-runtime/native/idna-nameprep.txt"
    with table.open("w", encoding="ascii", newline="\n") as output:
        generate(output)
