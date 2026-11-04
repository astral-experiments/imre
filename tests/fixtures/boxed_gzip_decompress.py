import gzip
from gzip import decompress


def current():
    first = gzip.compress(b"first")
    second = gzip.compress(b"second")
    assert decompress(b"") == b""
    assert gzip.decompress(first) == b"first"
    assert decompress(first + b"\0\0" + second + b"\0") == b"firstsecond"

    try:
        decompress(first + second[:-1])
    except EOFError:
        pass
    else:
        raise AssertionError("accepted a truncated trailer")

    bad_crc = bytearray(second)
    bad_crc[-8] ^= 1
    try:
        decompress(first + bad_crc)
    except gzip.BadGzipFile as exc:
        assert str(exc) == "CRC check failed"
    else:
        raise AssertionError("accepted a corrupt second member")
