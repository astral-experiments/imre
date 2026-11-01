import _io
import io

path = "imre-open-code.bin"
with open(path, "wb") as writer:
    writer.write(b"\x00\xff\r\ncode")

with _io.open_code(path) as reader:
    assert reader.read() == b"\x00\xff\r\ncode"
with io.open_code(path=path) as reader:
    assert reader.read(2) == b"\x00\xff"
    assert reader.read() == b"\r\ncode"

try:
    _io.open_code(b"imre-open-code.bin")
except TypeError:
    pass
else:
    raise AssertionError("open_code accepted a bytes path")

try:
    _io.open_code("missing-imre-open-code.bin")
except FileNotFoundError:
    pass
else:
    raise AssertionError("open_code opened a missing file")

print("binary code stream verified")
