import io
import os
from array import array


def current():
    path = "fileio-write.bin"
    try:
        with io.FileIO(path, "w+") as stream:
            words = array("h", [257, -2, 513])
            data = words.tobytes()
            assert stream.write(words) == len(data)
            with memoryview(words)[1:] as tail:
                assert stream.write(tail) == len(data) - words.itemsize

            source = io.BytesIO(b".ab.")
            with source.getbuffer()[1:3] as part:
                part[0] = ord("A")
                assert stream.write(part) == 2

            released = memoryview(b"x")
            released.release()
            position = stream.tell()
            for view, error in ((memoryview(b"abcd")[::2], BufferError), (released, ValueError)):
                try:
                    stream.write(view)
                except error:
                    assert stream.tell() == position
                else:
                    raise AssertionError("FileIO accepted an invalid view")
            stream.seek(0)
            assert stream.read() == data + data[words.itemsize:] + b"Ab"
    finally:
        os.unlink(path)
