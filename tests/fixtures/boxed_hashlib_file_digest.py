import hashlib
import io
import tempfile


def current():
    assert hashlib.file_digest.__name__ == "file_digest"
    assert hashlib.file_digest.__module__ == "hashlib"
    try:
        hashlib.file_digest(digest="sha256", fileobj=None)
    except TypeError as exc:
        assert str(exc) == (
            "file_digest() got some positional-only arguments passed as keyword arguments: 'fileobj, digest'"
        )
    else:
        raise AssertionError("accepted positional-only parameters by keyword")
    # The CLI test owns this temporary module directory.
    path = tempfile.mkdtemp(dir=".") + "/data"
    data = b"a" * 8193 + b"bc"
    with open(path, "wb") as f:
        f.write(data)
    seeded = hashlib.sha256(b"prefix")
    with open(path, "rb") as f:
        f.read(2)
        result = hashlib.file_digest(f, lambda: seeded, _bufsize=1000)
        assert result is seeded
        assert result.digest() == hashlib.sha256(b"prefix" + data[2:]).digest()
        assert f.read() == b""
    with open(path, "rb") as f:
        assert hashlib.file_digest(f, "sha1").digest() == hashlib.sha1(data).digest()
    stream = io.BytesIO(b"unaltered")
    stream.seek(5)
    assert hashlib.file_digest(stream, hashlib.sha256).digest() == hashlib.sha256(b"unaltered").digest()
    assert stream.tell() == 5
    stream.close()
    try:
        hashlib.file_digest(stream, "sha256")
    except ValueError:
        pass
    else:
        raise AssertionError("accepted closed BytesIO")
    with open(path, "r") as f:
        try:
            hashlib.file_digest(f, "sha256")
        except ValueError as exc:
            assert "is not a file-like object in binary reading mode" in str(exc)
        else:
            raise AssertionError("accepted text file")
    with open(path, "wb") as f:
        try:
            hashlib.file_digest(f, "sha256")
        except ValueError:
            pass
        else:
            raise AssertionError("accepted write-only file")

    class BufferSource:
        def getbuffer(self):
            return token

    class Consumer:
        def update(self, chunk):
            assert chunk is token
            self.called = True

    token = object()
    consumer = Consumer()
    assert hashlib.file_digest(BufferSource(), lambda: consumer) is consumer
    assert consumer.called
    try:
        hashlib.file_digest(None, "sha256")
    except ValueError as exc:
        assert str(exc) == "'None' is not a file-like object in binary reading mode."
    else:
        raise AssertionError("accepted None")
    try:
        hashlib.file_digest(None, "no-such-hash")
    except ValueError as exc:
        assert "unsupported hash type" in str(exc)
    else:
        raise AssertionError("file was checked before constructing digest")
    exported_reader()
    print("file digest")


def exported_reader():
    class Index:
        def __index__(self):
            # Hash the modifications made by this callback too.
            reader.saved[0] = 90
            return 2

    class Reader:
        def __init__(self, sizes):
            self.sizes = sizes
            self.saved = None
            self.index = 0

        def readable(self):
            return True

        def readinto(self, buf):
            assert type(buf) is bytearray
            if self.saved is not None:
                assert self.saved is buf
            self.saved = buf
            try:
                buf.append(65)
            except BufferError:
                pass
            else:
                raise AssertionError("resized an exported bytearray")
            for i in range(len(buf)):
                buf[i] = 65 + i
            result = self.sizes[self.index]
            self.index += 1
            return result

    reader = Reader([-1, 2 ** 100, Index(), 0])
    answer = hashlib.file_digest(reader, hashlib.sha256, _bufsize=4)
    assert answer.digest() == hashlib.sha256(b"ABCABCDZB").digest()
    reader.saved.append(65)
    reader = Reader([None])
    try:
        hashlib.file_digest(reader, "sha1", _bufsize=4)
    except BlockingIOError as exc:
        assert str(exc) == "I/O operation would block."
    else:
        raise AssertionError("accepted None as a byte count")
    reader.saved.append(65)
