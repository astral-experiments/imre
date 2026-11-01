import os
import posix


def current():
    assert os.stat is posix.stat and posix.stat_result is os.stat_result
    assert issubclass(os.stat_result, tuple)
    result = os.stat(path=__file__)
    assert type(result) is result.__class__ is os.stat_result
    assert len(result) == result.n_sequence_fields == 10
    assert result.st_size == result[6] and type(result.st_mtime) is float
    assert result.st_mtime_ns // 10**9 == result[8]
    assert "st_mtime_ns" in dir(result) and dir(result) == dir(os.stat_result)
    descriptor = os.stat_result.st_mtime
    assert descriptor is os.stat_result.st_mtime
    assert descriptor.__get__(result) == result.st_mtime
    assert descriptor.__get__(None, os.stat_result) is descriptor
    assert type(descriptor).__name__ == "member_descriptor"
    for method in (lambda: setattr(result, "st_atime", 1), lambda: delattr(result, "st_atime")):
        try:
            method()
        except AttributeError as error:
            assert str(error) == "readonly attribute"
        else:
            raise AssertionError("changed a read-only stat field")
    fd = os.open(__file__, os.O_RDONLY)
    try:
        assert result == os.stat(fd) == os.fstat(fd)
    finally:
        os.close(fd)
    assert os.lstat(__file__) == result
    made = os.stat_result(range(10), {"st_mtime": 8.25, "st_mtime_ns": 8250000000})
    assert made[8] == 8 and made.st_mtime == 8.25 and made.st_mtime_ns == 8250000000
    assert made.st_ctime == 9 and made.st_ctime_ns is None
    assert tuple(made) == tuple(range(10)) and hash(made) == hash(tuple(made))
    assert repr(made).endswith("st_atime=7, st_mtime=8, st_ctime=9)")
    for args in ((range(9),), (range(11), {"st_atime": 2}), (range(10), {"unknown": 3})):
        try:
            os.stat_result(*args)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError(args)
