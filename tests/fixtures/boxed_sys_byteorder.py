import struct
import sys
from sys import byteorder


def current():
    packed = struct.pack("=H", 0x0102)
    assert byteorder == ("little" if packed == b"\x02\x01" else "big")
    assert sys.byteorder is byteorder and sys.__dict__["byteorder"] is byteorder
    try:
        sys.byteorder = "changed"
        assert sys.byteorder == "changed" and byteorder != sys.byteorder
        del sys.byteorder
        assert not hasattr(sys, "byteorder")
    finally:
        sys.byteorder = byteorder
    print(sys.byteorder)
