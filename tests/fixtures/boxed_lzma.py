import io
import lzma
import zipfile
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


def raw_stream():
    default = {"id": lzma.FILTER_LZMA1}
    assert lzma._encode_filter_properties(default) == bytes.fromhex("5d00008000")
    settings = dict(default, dict_size=1 << 17, lc=2, lp=1, pb=1)
    props = lzma._encode_filter_properties(settings)
    assert props == bytes.fromhex("3800000200")
    assert lzma._decode_filter_properties(lzma.FILTER_LZMA1, props) == settings

    encoder = lzma.LZMACompressor(format=lzma.FORMAT_RAW, filters=[settings])
    payload = b"abcdef" * 512
    data = encoder.compress(payload[:19]) + encoder.compress(payload[19:]) + encoder.flush()
    decoder = lzma.LZMADecompressor(format=lzma.FORMAT_RAW, filters=[settings])
    assert decoder.decompress(data[:1]) == b"" and decoder.needs_input
    assert decoder.decompress(data[1:] + b"tail", max_length=11) == payload[:11]
    assert not decoder.eof and not decoder.needs_input
    assert decoder.decompress(b"") == payload[11:] and decoder.eof
    assert decoder.unused_data == b"tail"

    for error_type, operation, args in (
        (ValueError, encoder.compress, (b"",)),
        (ValueError, encoder.flush, ()),
        (EOFError, decoder.decompress, (b"",)),
        (LZMAError, lzma._decode_filter_properties, (lzma.FILTER_LZMA1, b"\xff" * 5)),
    ):
        try:
            operation(*args)
        except (ValueError, EOFError, LZMAError) as error:
            assert type(error) is error_type
        else:
            raise AssertionError("unexpected success")


def zip_members():
    # Stock CPython method 14, independent of the encoder being tested.
    stock = bytes.fromhex(
        "504b03043f0002000e000000212817dc3111370000006d0900000b0000007061"
        "796c6f61642e62696e090405005d00008000002d124643315e5a89cccd9c50fd"
        "ae53fcb7d3b7a79338d93a402cc2ae636cbe35d58e4a91c065ce5ffff43c8000"
        "504b01023f033f0002000e000000212817dc3111370000006d0900000b000000"
        "00000000000000008001000000007061796c6f61642e62696e504b0506000000"
        "000100010039000000600000000000"
    )
    payload = b"ZIP core / LZMA\x00\xff\r\n" * 127
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_LZMA) as archive:
        with archive.open("payload.bin", "w") as member:
            member.write(payload[:31])
            member.write(payload[31:])

    for data in (stock, output.getvalue()):
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            info = archive.getinfo("payload.bin")
            assert info.compress_type == 14 and info.CRC == 0x1131DC17
            assert archive.read(info) == payload
            assert archive.testzip() is None

    corrupt = bytearray(stock)
    corrupt[stock.index(b"PK\x01\x02") + 16] ^= 1  # Central directory CRC.
    with zipfile.ZipFile(io.BytesIO(corrupt)) as archive:
        assert archive.testzip() == "payload.bin"


if __name__ == "__main__":
    current()
    raw_stream()
    zip_members()
