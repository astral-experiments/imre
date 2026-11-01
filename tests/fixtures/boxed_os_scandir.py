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
    finally:
        os.close(fd)

    with os.scandir(os.fsencode(directory)) as entries:
        entry = next(item for item in entries if item.name == os.fsencode(name))
        assert entry.path == os.fsencode(os.path.join(directory, name))
