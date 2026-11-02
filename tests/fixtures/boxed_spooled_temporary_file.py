import io
import os
import tempfile
import warnings


def current():
    with tempfile.SpooledTemporaryFile(4, dir=".", prefix="spill-", suffix=".bin") as stream:
        assert isinstance(stream, io.IOBase)
        assert stream.write(b"abcd") == 4
        assert not stream._rolled and stream.name is None
        write = stream.write
        assert write(b"e") == 1 and stream._rolled
        assert stream.name == stream.fileno() and stream.mode == "rb+"
        descriptor = stream.fileno()
        os.close(os.dup(descriptor))
        stream.rollover()
        assert stream.fileno() == descriptor
        assert stream.tell() == 5 and stream.seek(2) == 2 and stream.read() == b"cde"
        assert stream.writelines([b"f", b"g"]) is None
        write_lines = stream._file.writelines
        assert write_lines((bytearray(b"h"),)) is None
        assert stream.seek(5) == 5 and stream.read() == b"fgh"
    assert stream.closed and stream.name == descriptor
    try:
        os.dup(descriptor)
    except OSError:
        pass
    else:
        raise AssertionError("the backing file descriptor survived close")

    with tempfile.SpooledTemporaryFile(4, dir=".") as stream:
        stream.write(b"abcdef")
        stream.seek(0)
        assert stream.read(2) == b"ab"
        assert stream.truncate() == stream.tell() == 2
        stream.seek(0)
        assert stream.read() == b"ab"
        assert stream.truncate(5) == 5 and stream.tell() == 2
        assert stream.read() == bytes(3)
        assert os.fstat(stream.fileno()).st_size == 5

    with tempfile.SpooledTemporaryFile(1, dir="missing-spool-directory") as memory:
        try:
            memory.write(b"retry")
        except FileNotFoundError:
            assert not memory._rolled
            memory.seek(0)
            assert memory.read() == b"retry"
        else:
            raise AssertionError("rolled into a missing directory")
    for action in ("always", "error"):
        spill = tempfile.SpooledTemporaryFile(dir=".")
        fd = spill.fileno()
        backing = spill._file
        with warnings.catch_warnings(record=True) as seen:
            warnings.simplefilter(action, ResourceWarning)
            try:
                spill.__del__()
            except ResourceWarning:
                assert action == "error"
        if action == "always":
            assert len(seen) == 1 and seen[0].source is spill
            assert backing.closed
        else:
            assert not seen and not backing.closed
            os.close(os.dup(fd))
        spill.close()
    print("checked boxed spooling and rollover")
