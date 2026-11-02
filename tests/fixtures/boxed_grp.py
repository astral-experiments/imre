import grp
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
    print("group records agree")
