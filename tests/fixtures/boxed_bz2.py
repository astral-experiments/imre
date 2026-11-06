import bz2
import io


def current():
    payload = b"archive record\n" * 100
    encoder = bz2.BZ2Compressor(1)
    encoded = encoder.compress(payload) + encoder.flush()
    decoder = bz2.BZ2Decompressor()
    assert decoder.decompress(encoded + b"tail", 7) == payload[:7]
    assert not decoder.needs_input
    assert decoder.decompress(b"") == payload[7:]
    assert decoder.eof and decoder.unused_data == b"tail"

    fileobj = io.BytesIO(encoded)
    fileobj.seek(0, 2)
    with bz2.BZ2File(fileobj, "w") as output:
        output.write(b"second member")
    fileobj.seek(0)
    with bz2.BZ2File(fileobj) as source:
        assert source.read() == payload + b"second member"
        assert source.seek(3) == 3
        assert source.read(7) == payload[3:10]
    assert not fileobj.closed
    try:
        with source:
            pass
    except ValueError:
        pass
    else:
        raise AssertionError("a closed BZ2File was entered")
