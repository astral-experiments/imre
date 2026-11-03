import os
from tempfile import TemporaryFile as anonymous


def check(buffering: int, before_flush: bytes) -> None:
    with anonymous(buffering=buffering) as stream:
        fd = stream.fileno()
        assert stream.write(b"abc") == 3
        assert os.pread(fd, 3, 0) == before_flush
        assert stream.tell() == 3
        stream.flush()
        assert os.pread(fd, 3, 0) == b"abc"


check(0, b"abc")
check(8, b"")
