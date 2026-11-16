import errno
import mmap
import os
import tempfile


def current():
    with tempfile.TemporaryFile() as stream:
        stream.write(b"response body")
        stream.flush()
        mapping = mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ)

    view = memoryview(mapping)
    tail = view[9:]
    mapping.seek(3)
    assert mapping.find(b"response") == -1
    assert mapping.find(tail) == mapping.rfind(tail) == 9
    assert mapping.find(b"", None, 0) == 3
    assert mapping.rfind(b"", 0, -1) == 12
    assert mapping.find(b"", 14) == 13
    assert mapping.rfind(b"", 4, 3) == -1
    assert mapping.tell() == 3
    view.release()
    assert tail.tobytes() == b"body" and tail.readonly
    with tempfile.TemporaryFile() as sink:
        assert os.write(sink.fileno(), tail) == 4
        assert os.pwrite(sink.fileno(), tail, 1) == 4
        sink.seek(0)
        assert sink.read() == b"bbody"
    try:
        mapping.close()
    except BufferError:
        pass
    else:
        raise AssertionError("closed a mapping with an exported view")
    tail.release()
    mapping.close()
    assert mapping.closed

    with mmap.mmap(-1, 8) as moving, memoryview(moving) as view:
        moving[:] = b"abcdefgh"
        moving.seek(3)
        assert moving.move(2, 0, 6) is None
        assert view.tobytes() == b"ababcdef"
        moving.move(0, 2, 6)
        assert view.tobytes() == b"abcdefef"
        assert moving.tell() == 3

    class CloseForSize:
        def __index__(self):
            flushing.close()
            return count

    for offset, count, expected in ((0, 0, None), (0, 1, errno.ENOMEM), (1, 1, errno.EINVAL)):
        flushing = mmap.mmap(-1, mmap.PAGESIZE)
        try:
            assert flushing.flush(offset, CloseForSize()) is None
        except OSError as error:
            assert expected is not None and error.errno == expected
        else:
            assert expected is None
        assert flushing.closed

    with tempfile.TemporaryFile() as source:
        source.write(b"abc")
        source.flush()
        for access, byte in ((mmap.ACCESS_WRITE, b"W"), (mmap.ACCESS_COPY, b"C")):
            with mmap.mmap(source.fileno(), 0, access=access) as mapped:
                with memoryview(mapped) as view:
                    view[1] = byte[0]
                assert mapped[:] == b"a" + byte + b"c"
                mapped.flush()
            source.seek(0)
            assert source.read() == b"aWc"


def limits():
    import errno
    import resource

    size = 64 << 20
    with tempfile.TemporaryFile() as source:
        source.truncate(size)
        with mmap.mmap(source.fileno(), size) as mapping:
            mapping[0] = 65
            with open("/proc/self/statm") as status:
                used = int(status.read().split()[0]) * mmap.PAGESIZE
            soft, hard = resource.getrlimit(resource.RLIMIT_AS)
            cap = used + (6 << 20)
            if soft != resource.RLIM_INFINITY:
                cap = min(cap, soft)
            resource.setrlimit(resource.RLIMIT_AS, (cap, hard))
            try:
                for step in (1, 2, -1):
                    try:
                        mapping[::step]
                    except MemoryError as error:
                        assert error.args == ()
                    else:
                        raise AssertionError("slice exceeded its address-space limit")
                assert mapping[1:0:2] == b""
                assert mapping[::size] == b"A"
                mapping.seek(1)
                try:
                    mapping.write(mapping)
                except ValueError as error:
                    assert str(error) == "data out of range"
                else:
                    raise AssertionError("accepted an oversized mapped source")
                assert mapping.tell() == 1 and mapping[0] == 65
                mapping.seek(0)
                assert mapping.write(mapping) == size
                try:
                    mapping[:1] = mapping
                except IndexError:
                    pass
                else:
                    raise AssertionError("accepted a mismatched mapping")
            finally:
                resource.setrlimit(resource.RLIMIT_AS, (soft, hard))

        soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
        resource.setrlimit(resource.RLIMIT_NOFILE, (3, hard))
        try:
            for length, offset in ((size + 1, 0), (0, size)):
                try:
                    mmap.mmap(source.fileno(), length, offset=offset)
                except ValueError:
                    pass
                else:
                    raise AssertionError("accepted an invalid file extent")
            try:
                mmap.mmap(source.fileno(), size)
            except OSError as error:
                assert error.errno == errno.EMFILE
            else:
                raise AssertionError("tracked mapping needs a descriptor")
        finally:
            resource.setrlimit(resource.RLIMIT_NOFILE, (soft, hard))


def assignment_callbacks():
    class Key:
        def __index__(self):
            target.close()
            return 1

    class Value:
        def __index__(self):
            sibling[0] = ord("V")
            raise LookupError("value wins")

    class Source:
        def __buffer__(self, flags):
            sibling[0] = ord("G")
            return memoryview(b"x")

        def __release_buffer__(self, view):
            sibling[1] = ord("R")

    with tempfile.TemporaryFile() as file:
        file.write(b"abcd")
        file.flush()
        with mmap.mmap(file.fileno(), 0) as sibling:
            with mmap.mmap(file.fileno(), 0) as target:
                try:
                    target[Key()] = Value()
                except LookupError as error:
                    assert str(error) == "value wins"
                else:
                    raise AssertionError("skipped right-hand integer conversion")
            assert sibling[:] == b"Vbcd"
            with mmap.mmap(file.fileno(), 0) as target:
                try:
                    target[Key():3] = Source()
                except IndexError as error:
                    assert str(error) == "mmap slice assignment is wrong size"
                else:
                    raise AssertionError("accepted a mismatched source buffer")
            assert sibling[:] == b"GRcd"
