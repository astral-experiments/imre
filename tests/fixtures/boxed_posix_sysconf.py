import errno
import os
import posix
from posix import sysconf as query


class PageSize:
    def __index__(self):
        return posix.sysconf_names["SC_PAGESIZE"]


def current():
    names = posix.sysconf_names
    assert query is os.sysconf and names is os.sysconf_names
    page_size = query("SC_PAGESIZE")
    assert page_size > 0 and page_size == query("SC_PAGE_SIZE") == query(PageSize())
    assert query("SC_OPEN_MAX") == query(names["SC_OPEN_MAX"]) > 0

    for value, error_type in (
        ("not a configuration name", ValueError),
        (b"SC_PAGESIZE", TypeError),
        (1 << 60, OverflowError),
        (-1, OSError),
    ):
        try:
            query(value)
        except (ValueError, TypeError, OverflowError, OSError) as error:
            assert type(error) is error_type
            if error_type is OSError:
                assert error.errno == errno.EINVAL
            print(type(error).__name__, str(error))
        else:
            raise AssertionError(value)
    # An indeterminate limit can return -1 even immediately after an invalid ID.
    print("SC_AIO_MAX", query("SC_AIO_MAX"))

    try:
        names["my_pages"] = names["SC_PAGESIZE"]
        assert query("my_pages") == page_size
        os.sysconf_names = {}
        assert query("my_pages") == page_size
        posix.sysconf_names = {"replacement": names["SC_PAGESIZE"]}
        assert os.sysconf("replacement") == page_size
        try:
            query("my_pages")
        except ValueError as error:
            assert str(error) == "unrecognized configuration name"
        else:
            raise AssertionError("sysconf ignored its module's replacement mapping")
    finally:
        posix.sysconf_names = names
        os.sysconf_names = names
        del names["my_pages"]
