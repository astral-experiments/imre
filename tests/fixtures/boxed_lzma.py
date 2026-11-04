import lzma
from lzma import LZMAError


def current():
    # XZ and legacy LZMA-alone streams compressed with preset 1.
    xz = bytes.fromhex(
        "fd377a585a000004e6d6b4460200210110000000a8708e86e1d4bf005d5d0024"
        "9b4a46637dff5d53db48bc629064ce8a107a2265cb621cabd9dc5ef1ef8164"
        "2752e6210b23e9f362c73feabeaa4b36daece26db0ea43055ec6c3ce71bce5"
        "53d3e059f37683b10fe26aa74c82abea4fa308afca330c83c6f92e345600000"
        "000001e9aaf5166833e32000179c0a907000005204133b1c467fb020000000004595a"
    )
    alone = bytes.fromhex("5d00001000ffffffffffffffff0039994891b169953d46a4fffffe73b000")
    expected = b"Imre\x00\xff" * 20000
    assert lzma.decompress(xz) == expected
    assert lzma.decompress(alone) == b"second\x00"
    assert lzma.decompress(xz + alone) == expected + b"second\x00"
    assert lzma.decompress(xz + b"no valid header here") == expected
    assert lzma.decompress(xz, memlimit=16 << 20) == expected
    print("XZ, LZMA-alone, concatenation, trailing data and memory limit passed")

    # Flip one byte in the XZ integrity check.
    corrupt = xz[:-26] + bytes([xz[-26] ^ 1]) + xz[-25:]
    truncated = "Compressed data ended before the end-of-stream marker was reached"
    for label, stream, limit, message in (
        ("corrupt", corrupt, None, "Corrupt input data"),
        ("truncated", xz[:-1], None, truncated),
        ("truncated second stream", xz + alone[:-1], None, truncated),
        ("memory limit", xz, 4096, "Memory usage limit exceeded"),
    ):
        try:
            lzma.decompress(stream, memlimit=limit)
        except LZMAError as error:
            assert type(error) is lzma.LZMAError
            assert str(error) == message
            print(label, str(error))
        else:
            raise AssertionError(label)


if __name__ == "__main__":
    current()
