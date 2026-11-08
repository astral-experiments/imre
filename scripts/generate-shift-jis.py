"""Generate a packed double-byte decoding table from CPython 3.14.5.

The public codecs use Modules/cjkcodecs/_codecs_jp.c and _codecs_tw.c at
5607950ef232dad16d75c0cf53101d9649d89115 (see licenses/CPython.txt).
Each entry is a little-endian 16-bit Unicode scalar, or U+FFFE for a hole.
"""
import argparse
from pathlib import Path
import sys


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("Run with CPython 3.14.5")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--encoding", choices=("shift_jis", "big5"), default="shift_jis")
    args = parser.parse_args()

    if args.encoding == "big5":
        leads = range(0xA1, 0xFA)
        upper_trails = range(0xA1, 0xFF)
    else:
        leads = (*range(0x81, 0xA0), *range(0xE0, 0xEB))
        upper_trails = range(0x80, 0xFD)
    trails = (*range(0x40, 0x7F), *upper_trails)
    table = bytearray()
    reverse = {}
    for lead in leads:
        for trail in trails:
            pair = bytes((lead, trail))
            try:
                scalar = pair.decode(args.encoding)
            except UnicodeDecodeError:
                value = 0xFFFE
            else:
                value = ord(scalar)
                reverse[scalar] = pair
            table.extend(value.to_bytes(2, "little"))
    for scalar, pair in reverse.items():
        if scalar.encode(args.encoding) != pair:
            raise ValueError(f"noncanonical reverse mapping for {scalar!r}")
    args.output.write_bytes(table)


if __name__ == "__main__":
    main()
