import errno
import os
import sys
import tempfile
import warnings


def current():
    if sys.platform != "linux":
        return
    import posix

    assert os.pathconf is posix.pathconf and os.fpathconf is posix.fpathconf
    names = os.pathconf_names
    assert names is posix.pathconf_names
    key = names["PC_NAME_MAX"]
    dir_fd = os.open(".", os.O_RDONLY)
    try:
        assert os.fpathconf(dir_fd, key) == os.pathconf(".", "PC_NAME_MAX")
    finally:
        os.close(dir_fd)
    with tempfile.NamedTemporaryFile() as file:
        value = os.pathconf(path=file.name, name=key)
        assert value > 0 and value == os.pathconf(os.fsencode(file.name), "PC_NAME_MAX")
        fd = file.fileno()
        assert value == os.pathconf(fd, key) == os.fpathconf(file, key)
        with warnings.catch_warnings(record=True) as seen:
            warnings.simplefilter("always")
            for call in (os.fpathconf, os.pathconf):
                call(False, "PC_MAX_CANON")
            assert [str(w.message) for w in seen] == ["bool is used as a file descriptor"] * 2
        assert os.pathconf in os.supports_fd
        names["PC_IMRE_CUSTOM"] = key
        try:
            assert os.fpathconf(fd, "PC_IMRE_CUSTOM") == value
        finally:
            del names["PC_IMRE_CUSTOM"]
    if "PC_SYMLINK_MAX" in names:
        # The kernel may report no fixed maximum with errno zero.
        assert os.pathconf(".", "PC_SYMLINK_MAX") >= -1
    for call, arg, number, expected_filename in (
        (os.pathconf, fd, key, fd),
        (os.fpathconf, fd, key, None),
        (os.pathconf, -1, 1, -1),
        (os.pathconf, ".", 1000000, None),
    ):
        try:
            result = call(arg, number)
        except OSError as exc:
            assert exc.errno == (errno.EINVAL if arg == "." else errno.EBADF)
            assert exc.filename == expected_filename
            print("errno", exc.errno)
        else:
            # Some libcs return configuration constants without consulting the fd.
            assert arg != "." and result >= -1
            print("limit", result)
