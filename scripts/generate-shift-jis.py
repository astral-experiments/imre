"""Generate the Shift JIS double-byte decoding table with pinned CPython 3.14.5.

The source is the public shift_jis codec in CPython v3.14.5, commit
5607950ef232dad16d75c0cf53101d9649d89115 (see licenses/CPython.txt).
Supply the output filename as the single command-line argument. Each entry
is a little-endian 16-bit Unicode scalar, or U+FFFE for an undefined pair.
"""
from pathlib import Path
import sys


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("Run with CPython 3.14.5")

    # Rows and columns follow the encoded byte order, omitting the gaps.
    leads = (*range(0x81, 0xA0), *range(0xE0, 0xEB))
    trails = (*range(0x40, 0x7F), *range(0x80, 0xFD))
    table = bytearray()
    for lead in leads:
        for trail in trails:
            try:
                value = ord(bytes((lead, trail)).decode("shift_jis"))
            except UnicodeDecodeError:
                value = 0xFFFE
            table.extend(value.to_bytes(2, "little"))
    Path(sys.argv[1]).write_bytes(table)


if __name__ == "__main__":
    main()
