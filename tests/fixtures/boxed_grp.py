import grp
import sys
from grp import getgrall, getgrgid, getgrnam, struct_group


def current():
    assert getgrgid is grp.getgrgid
    rows = getgrall()
    assert type(rows) is list
    for row in rows:
        assert type(row) is struct_group and isinstance(row, tuple)
        assert tuple(row) == (row.gr_name, row.gr_passwd, row.gr_gid, row.gr_mem)
        assert isinstance(row.gr_mem, list)
        if row.gr_name and row.gr_name[0] not in "+-":
            assert getgrnam(name=row.gr_name).gr_name.lower() == row.gr_name.lower()
            assert getgrgid(id=row.gr_gid).gr_gid == row.gr_gid
            break

    class GroupID:
        def __index__(self):
            return -2

    for function, value, error in (
        (getgrgid, GroupID(), OverflowError),
        (getgrgid, "0", TypeError),
        (getgrnam, b"root", TypeError),
        (getgrnam, "bad\0name", ValueError),
    ):
        try:
            function(value)
        except error:
            pass
        else:
            raise AssertionError((function, value))
    if sys.platform == "linux":
        import os
        import posix
        import pwd

        user = pwd.getpwuid(os.getuid())
        lookup = os.getgrouplist
        assert vars(os)["getgrouplist"] is vars(posix)["getgrouplist"] is lookup
        assert lookup.__self__ is posix
        groups = lookup(user.pw_name, user.pw_gid)
        assert type(groups) is list and user.pw_gid in groups

        class Base:
            def __index__(self):
                return -1

        assert -1 in lookup(user.pw_name, Base())
        for arguments, error in (
            ((b"bad", 0), TypeError),
            (("bad\0name", 0), ValueError),
            ((chr(0xDC80), 0), UnicodeEncodeError),
            ((user.pw_name, -2), OverflowError),
            ((user.pw_name, 1 << 64), OverflowError),
        ):
            try:
                lookup(*arguments)
            except error:
                pass
            else:
                raise AssertionError(arguments)

    print("group records agree")
