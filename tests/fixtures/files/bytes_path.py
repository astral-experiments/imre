import errno
import io


class Filename:
    def __fspath__(self):
        return b"imre-raw-bytes-utf8-\xc3\xa9"


name = Filename()
with io.FileIO(name, "w") as writer:
    assert writer.name is name
    assert writer.write(b"abc\x00\xff") == 5

with io.FileIO(b"imre-raw-bytes-utf8-\xc3\xa9", "r") as reader:
    assert reader.name == b"imre-raw-bytes-utf8-\xc3\xa9"
    assert reader.read() == b"abc\x00\xff"

raw = b"imre-raw-bytes-\xff"
try:
    file = io.FileIO(raw, "w")
except OSError as exc:
    # Some filesystems reject non-UTF-8 directory entries at creation.
    assert exc.errno == errno.EILSEQ
    assert exc.filename == raw
else:
    with file:
        assert file.write(b"\xff") == 1

try:
    io.FileIO(b"imre-raw-nul\x00suffix", "w")
except ValueError:
    pass
else:
    raise AssertionError("FileIO accepted an embedded NUL")

print("raw bytes filename verified")
