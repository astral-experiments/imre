import hashlib
import io
from hashlib import file_digest, sha256


def buffer() -> io.BytesIO:
    print("stream")
    return io.BytesIO(b"abc")


def algorithm() -> str:
    print("name")
    return "SHA-1"


def size() -> list[int]:
    print("size")
    return [1, 2]


def selected(name: str) -> str:
    return file_digest(io.BytesIO(b"abc"), name).hexdigest()


def bad_size() -> int:
    raise RuntimeError("size argument failed")


print(file_digest(buffer(), algorithm(), _bufsize=size()).hexdigest())
print(file_digest(io.BytesIO(), hashlib.sha1, _bufsize=None).hexdigest())
stream = io.BytesIO(b"prefix\0payload")
stream.seek(7)
state = file_digest(stream, sha256)
print(stream.tell())
print(state.hexdigest())
state.update(b"suffix")
print(state.hexdigest())
stream.close()
for name in ["invalid", "sha256"]:
    try:
        file_digest(stream, name)
    except ValueError as exc:
        print(str(exc))

print(selected("sha1"))
try:
    file_digest(stream, "invalid", _bufsize=bad_size())
except RuntimeError as exc:
    print(str(exc))

with open("chunks.bin", "wb") as writer:
    writer.write(b"prefix" + bytes(524291) + b"end")
    try:
        file_digest(writer, "sha256", _bufsize=-1)
    except ValueError as exc:
        print(str(exc))

with open("chunks.bin", "rb") as reader:
    print(reader.read(6))
    print(file_digest(reader, "sha1", _bufsize=0).hexdigest())
    print(reader.tell())
    print(file_digest(reader, sha256, _bufsize=8193).hexdigest())
    print(reader.tell())
    print(file_digest(reader, "sha256").hexdigest())
    try:
        file_digest(reader, "sha256", _bufsize=-1)
    except ValueError as exc:
        print(str(exc))
    try:
        file_digest(reader, "sha256", _bufsize=2**100)
    except OverflowError as exc:
        print(str(exc))

for digest_name in ["invalid", "sha1"]:
    try:
        file_digest(reader, digest_name, _bufsize=-1)
    except ValueError as exc:
        print(str(exc))
try:
    file_digest(None, "sha256")
except ValueError as exc:
    print(str(exc))

with open("chunks.bin", "rb") as reader:
    print(file_digest(reader, hashlib.sha224).hexdigest())
    reader.seek(6)
    print(file_digest(reader, hashlib.sha384).hexdigest())
print(file_digest(io.BytesIO(b"abc"), hashlib.sha512).hexdigest())
