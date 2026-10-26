import os
from pathlib import Path
from tempfile import SpooledTemporaryFile


def upload(size: int) -> None:
    limit = 500 * 1024
    part = (b"\x00\x80\xffx" * (size // 4 + 1))[:size]
    with SpooledTemporaryFile(max_size=limit, mode="rb+") as stream:
        # MultiPartParser keeps the bound write method across its data events.
        write = stream.write
        seek = stream.seek
        read = stream.read
        for start in range(0, size, 64 * 1024):
            chunk = part[start:start + 64 * 1024]
            assert write(chunk) == len(chunk)
        assert stream.tell() == size
        assert stream._rolled == (size > limit)
        if not stream._rolled:
            assert stream.name is None
        else:
            os.close(os.dup(stream.fileno()))
        assert seek(0, 0) == 0
        assert read(size) == part
    assert stream.closed


for length in (0, 17, 511999, 512000, 512001, 577537):
    upload(length)

spool = SpooledTemporaryFile(max_size=8)
assert spool.write(b"abcdefgh") == 8
assert spool.seek(1) == 1
assert spool.write(b"Z") == 1
assert not spool._rolled
spool.rollover()
fd = spool.fileno()
assert spool.fileno() == fd and spool.name == fd
os.close(os.dup(fd))
assert spool.tell() == 2 and spool.mode == "rb+"
assert spool.read() == b"cdefgh"
spool.rollover()
assert spool.seek(0) == 0 and spool.read() == b"aZcdefgh"
spool.close()
spool.close()
assert spool.name == fd
try:
    os.dup(fd)
except OSError:
    pass
else:
    raise AssertionError("closed spool kept its file descriptor")

with SpooledTemporaryFile() as unlimited:
    assert unlimited.seek(26) == 26
    assert unlimited.write(b"!") == 1 and not unlimited._rolled
    assert unlimited.fileno() >= 0 and unlimited._rolled
    assert unlimited.seek(0) == 0 and unlimited.read() == b"\0" * 26 + b"!"

try:
    with unlimited:
        raise AssertionError("entered a closed spool")
except ValueError as exc:
    print(str(exc))

# The triggering write lands in memory even if creating the backing file fails.
with SpooledTemporaryFile(max_size=1, dir="missing-spool-dir") as retry:
    try:
        retry.write(b"ab")
    except FileNotFoundError:
        assert not retry._rolled and retry.tell() == 2
    else:
        raise AssertionError("rolled into a nonexistent directory")
    Path("missing-spool-dir").mkdir()
    try:
        retry.rollover()
        assert retry._rolled and retry.tell() == 2
        assert retry.seek(0) == 0 and retry.read() == b"ab"
    finally:
        Path("missing-spool-dir").rmdir()
