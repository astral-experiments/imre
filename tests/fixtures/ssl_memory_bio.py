import _ssl
import ssl
from ssl import MemoryBIO

assert MemoryBIO is ssl.MemoryBIO is _ssl.MemoryBIO
assert ssl.SSLError is _ssl.SSLError
bio = MemoryBIO()
source = bytearray(b"abc")
assert bio.write(memoryview(source)) == 3
source[:] = b"xyz"
assert bio.read(1) == b"a" and bio.pending == 2
bio.write_eof()
assert not bio.eof
try:
    bio.write(b"")
except ssl.SSLError as error:
    assert error.args == ("cannot write() after write_eof()",)
else:
    raise AssertionError("write after write_eof succeeded")
assert bio.read() == b"bc" and bio.pending == 0 and bio.eof
assert bio.read() == b""
