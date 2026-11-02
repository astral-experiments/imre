import os
import posix
import sys


def check_reduction(result):
    descriptor = os.stat_result.__reduce__
    assert type(descriptor).__name__ == "method_descriptor"
    assert descriptor.__objclass__ is os.stat_result
    for method in (descriptor, descriptor.__get__(result)):
        assert method.__qualname__ == "stat_result.__reduce__"
        assert method.__doc__ is None and method.__text_signature__ == "($self, /)"
    shared = []
    made = os.stat_result([shared] * 10, {"st_atime_ns": shared})
    names = ("st_atime st_mtime st_ctime st_atime_ns st_mtime_ns st_ctime_ns "
             "st_blksize st_blocks st_rdev st_flags st_gen st_birthtime").split()
    for value in (result, made):
        constructor, (items, fields) = value.__reduce__()
        assert constructor is os.stat_result
        assert type(items) is tuple and len(items) == 10
        assert list(fields) == [name for name in names if hasattr(value, name)]
        assert len(fields) == value.n_fields - len(items)
        restored = constructor(items, fields)
        for i in range(len(items)):
            assert restored[i] is items[i] is value[i]
        for name, item in fields.items():
            assert getattr(restored, name) is item is getattr(value, name)
        _, (_, other_fields) = os.stat_result.__reduce__(value)
        assert other_fields == fields and other_fields is not fields
    assert made.st_atime_ns is shared and made.st_mtime_ns is None
    try:
        os.stat_result.__reduce__(())
    except TypeError:
        pass
    else:
        raise AssertionError("stat_result reducer accepted a plain tuple")


def check_bool_fd_warning():
    import warnings

    with warnings.catch_warnings(record=True) as seen:
        warnings.simplefilter("always", RuntimeWarning)
        os.stat(os.path.abspath(__file__), dir_fd=False)
    assert len(seen) == 1 and seen[0].category is RuntimeWarning
    assert seen[0].filename == __file__
    assert str(seen[0].message) == "bool is used as a file descriptor"


def check_statvfs():
    with open(__file__) as stream:
        fd = stream.fileno()
        path = os.statvfs(__file__)
        for result in (os.fstatvfs(fd), posix.statvfs(fd)):
            assert type(result) is os.statvfs_result is posix.statvfs_result
            assert len(result) == 10 and result.f_bfree == result[3]
            assert (result.f_fsid, result.f_bsize) == (path.f_fsid, path.f_bsize)
        assert os.statvfs in os.supports_fd
    try:
        os.fstatvfs(fd)
    except OSError as error:
        import errno
        assert error.errno == errno.EBADF and error.filename is None
    else:
        raise AssertionError("closed fd accepted")
    result = os.statvfs_result(range(10), {"f_fsid": 2**64 - 1})
    assert result.__reduce__() == (os.statvfs_result, (tuple(range(10)), {"f_fsid": result.f_fsid}))
    assert result.f_fsid == 2**64 - 1 and os.statvfs_result(range(10)).f_fsid is None


def current():
    if sys.platform == "linux":
        check_statvfs()
    assert os.stat is posix.stat and posix.stat_result is os.stat_result
    assert issubclass(os.stat_result, tuple)
    result = os.stat(path=__file__)
    check_reduction(result)
    check_bool_fd_warning()
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
