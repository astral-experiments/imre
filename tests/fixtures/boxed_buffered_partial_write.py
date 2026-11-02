import errno
import io
import os


def fill(fd):
    total = 0
    while True:
        try:
            total += os.write(fd, b"x" * 4096)
        except BlockingIOError:
            return total


def drain(fd):
    parts = []
    while True:
        try:
            data = os.read(fd, 65536)
        except BlockingIOError:
            break
        if not data:
            break
        parts.append(data)
    return b"".join(parts)


def blocked(method, *args):
    try:
        method(*args)
    except BlockingIOError as error:
        count = error.characters_written
        assert error.args == (errno.EAGAIN, "write could not complete without blocking", count)
        return count
    raise AssertionError("nonblocking operation unexpectedly completed")


def flush_bytes(stream, reader):
    parts = []
    while True:
        try:
            stream.flush()
        except BlockingIOError as error:
            assert error.characters_written == 0
            chunk = drain(reader)
            assert chunk
            parts.append(chunk)
        else:
            parts.append(drain(reader))
            return b"".join(parts)


def current():
    r, w = os.pipe()
    try:
        os.set_blocking(r, False)
        os.set_blocking(w, False)
        with open(w, "wb", closefd=False) as stream:
            capacity = max(io.DEFAULT_BUFFER_SIZE, stream.raw._blksize)
            payload = b"A" * capacity
            assert stream.write(payload) == capacity
            early = drain(r)
            assert early and early + flush_bytes(stream, r) == payload
            assert stream.write(b"x") == 1 and drain(r) == b""
            assert stream.write(payload) == capacity
            early = drain(r)
            assert len(early) > 1 and early + flush_bytes(stream, r) == b"x" + payload

            pad = fill(w)
            assert stream.write(payload[:-2]) == capacity - 2
            assert blocked(stream.write, b"12345") == 2
            assert blocked(stream.flush) == 0
            assert drain(r) == b"x" * pad
            assert flush_bytes(stream, r) == payload[:-2] + b"12"

            pad = fill(w)
            assert os.read(r, 4096) == b"x" * 4096
            data = b"A" * 4096 + b"B" * capacity + b"C" * 4096
            count = blocked(stream.write, data)
            received = drain(r)
            prefix = b"x" * (pad - 4096)
            assert received[:len(prefix)] == prefix
            direct = len(received) - len(prefix)
            assert received[len(prefix):] == data[:direct] and count == direct + capacity
            assert flush_bytes(stream, r) == data[direct:count]
            assert stream.write(data[count:]) == len(data) - count
            assert flush_bytes(stream, r) == data[count:]

            pad = fill(w)
            assert stream.write(payload[:-2]) == capacity - 2
            assert os.read(r, 4096) == b"x" * 4096
            count = blocked(stream.write, b"B" * 8192)
            received = drain(r)
            prefix = b"x" * (pad - 4096)
            assert received[:len(prefix)] == prefix
            direct = len(received) - len(prefix)
            assert received[len(prefix):] == b"A" * direct and count == direct + 2
            assert flush_bytes(stream, r) == payload[direct:-2] + b"B" * count
        stream.close()
        assert os.write(w, b"open") == 4 and drain(r) == b"open"
        owned = os.dup(w)
        other = open(owned, "wb")
        other.close()
        other.close()
        try:
            os.write(owned, b"x")
        except OSError as error:
            assert error.errno == errno.EBADF
        else:
            raise AssertionError("owned descriptor remained open")
    finally:
        os.close(w)
        os.close(r)
