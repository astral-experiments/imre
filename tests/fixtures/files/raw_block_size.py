import io

with io.FileIO("raw-block-size", "w") as stream:
    initial = stream._blksize
    assert isinstance(initial, int) and initial > 1
    try:
        stream._blksize = 1
    except AttributeError:
        pass
    else:
        raise AssertionError("FileIO._blksize must be read-only")
    assert stream.write(b"a") == 1
    assert stream._blksize == initial
    borrowed = io.FileIO(stream.fileno(), "w", closefd=False)
    assert borrowed._blksize == initial
    borrowed.close()
    assert borrowed._blksize == initial
    try:
        borrowed.__init__(stream.fileno(), "invalid")
    except ValueError:
        pass
    else:
        raise AssertionError("FileIO accepted an invalid mode")
    assert borrowed._blksize == io.DEFAULT_BUFFER_SIZE
    assert stream.truncate(1) == 1
    assert stream._blksize == io.DEFAULT_BUFFER_SIZE

with io.FileIO("raw-block-size", "r") as stream:
    assert stream._blksize == initial
assert stream._blksize == io.DEFAULT_BUFFER_SIZE
print("FileIO block size verified")
