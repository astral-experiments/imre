import os
import tempfile


with tempfile.TemporaryFile("w+b", 0) as stream:
    assert stream.write(b"raw") == 3
    assert os.pread(stream.fileno(), 3, 0) == b"raw"

try:
    tempfile.TemporaryFile("w+", 0)
except ValueError as exc:
    assert str(exc) == "can't have unbuffered text I/O"
else:
    raise AssertionError("unbuffered text stream opened")
