from tempfile import TemporaryFile


with TemporaryFile(mode="w+", encoding="latin-1", newline="\r\n") as stream:
    assert stream.write("café\n") == 5
    stream.flush()
    stream.seek(0)
    assert stream.buffer.read() == b"caf\xe9\r\n"
assert stream.closed
