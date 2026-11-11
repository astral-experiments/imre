import os
from pathlib import Path

path = Path("touched")
assert path.touch() is None
path.write_bytes(b"original")
os.utime(path, ns=(1_000_000_000, 1_000_000_000))
path.touch()
assert path.stat().st_mtime_ns > 1_000_000_000
assert path.read_bytes() == b"original"

try:
    path.touch(exist_ok=False)
except FileExistsError as error:
    assert error.filename == "touched"
else:
    raise AssertionError("exclusive touch accepted an existing file")

created = Path("exclusive")
mask = os.umask(0o027)
try:
    created.touch(0o765, False)
finally:
    os.umask(mask)
assert created.stat().st_mode & 0o777 == 0o740
