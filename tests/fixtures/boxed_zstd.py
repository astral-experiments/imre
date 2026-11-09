import io
from compression import zstd


def current():
    records = b"tar member contents\n" * 100
    first = zstd.compress(records)
    second = zstd.compress(b"next frame")
    decoder = zstd.ZstdDecompressor()
    assert decoder.decompress(first + second, 7) == records[:7]
    assert not decoder.needs_input
    assert decoder.decompress(b"") == records[7:]
    assert decoder.eof and decoder.unused_data == second

    source = io.BytesIO(first + second)
    with zstd.ZstdFile(source) as reader:
        assert reader.read() == records + b"next frame"
    assert not source.closed
    try:
        zstd.decompress(first[:-1])
    except zstd.ZstdError:
        pass
    else:
        raise AssertionError("accepted an incomplete Zstandard frame")
