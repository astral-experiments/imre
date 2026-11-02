import os
import posix


def current():
    directory = os.path.dirname(__file__) or "."
    name = os.path.basename(__file__)
    assert os.scandir is posix.scandir and os.scandir in os.supports_fd
    fd = os.open(directory, os.O_RDONLY)
    try:
        with os.scandir(fd) as entries:
            assert iter(entries) is entries
            entry = next(item for item in entries if item.name == name)
        assert list(entries) == []
        assert type(entry) is os.DirEntry and entry.path == name
        assert os.fspath(entry) is entry.path
        assert os.DirEntry.name.__get__(entry) is entry.name
        assert os.DirEntry.stat.__objclass__ is os.DirEntry
        st = entry.stat()
        assert st == os.stat(name, dir_fd=fd)
        assert entry.stat(follow_symlinks=False) is st
        assert entry.inode() == st.st_ino and entry.is_file()
        check_reduction(entry)
    finally:
        os.close(fd)

    with os.scandir(os.fsencode(directory)) as entries:
        entry = next(item for item in entries if item.name == os.fsencode(name))
        assert entry.path == os.fsencode(os.path.join(directory, name))


def check_reduction(entry):
    import copyreg
    import pickle

    assert os.DirEntry.__reduce__ is object.__reduce__
    assert os.DirEntry.__reduce_ex__ is object.__reduce_ex__
    expected = (copyreg._reconstructor, (os.DirEntry, object, None))
    assert entry.__reduce__() == expected
    for protocol in (0, 1):
        assert entry.__reduce_ex__(protocol) == expected
        data = pickle.dumps(entry, protocol)
        assert b"ccopy_reg\n_reconstructor\n" in data
        try:
            pickle.loads(data)
        except TypeError as error:
            assert str(error) == "cannot create 'posix.DirEntry' instances"
        else:
            raise AssertionError("constructed a directory entry")
    for protocol in range(2, pickle.HIGHEST_PROTOCOL + 1):
        for invoke in (lambda: entry.__reduce_ex__(protocol), lambda: pickle.dumps(entry, protocol)):
            try:
                invoke()
            except TypeError as error:
                assert str(error) == "cannot pickle 'posix.DirEntry' object"
            else:
                raise AssertionError("accepted a directory entry")
