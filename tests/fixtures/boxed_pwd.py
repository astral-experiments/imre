import os
import posix
import pwd
from pwd import getpwall, getpwnam, getpwuid, struct_passwd


def current():
    assert vars(os)["geteuid"] is vars(posix)["geteuid"] is os.geteuid
    assert type(os.geteuid()) is int
    assert vars(pwd)["getpwall"] is getpwall
    fields = ("pw_name", "pw_passwd", "pw_uid", "pw_gid", "pw_gecos", "pw_dir", "pw_shell")
    entries = getpwall()
    assert type(entries) is list
    for entry in entries:
        assert type(entry) is struct_passwd and isinstance(entry, tuple)
        assert tuple(entry) == tuple(getattr(entry, field) for field in fields)
        assert entry == struct_passwd(entry)
    for entry in entries[:4]:
        if not entry.pw_name or entry.pw_name == "+":
            continue
        assert getpwnam(entry.pw_name).pw_name == entry.pw_name
        assert getpwuid(entry.pw_uid).pw_uid == entry.pw_uid
    for function, value, error in ((getpwuid, -2, KeyError), (getpwuid, "0", TypeError),
                                    (getpwnam, "a\0b", ValueError), (getpwnam, b"user", TypeError)):
        try:
            function(value)
        except error:
            pass
        else:
            raise AssertionError(function.__name__)
    print("passwd records agree")
