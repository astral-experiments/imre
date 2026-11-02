import os
import posix


def current():
    assert os.makedev is posix.makedev
    assert os.major is posix.major
    assert os.minor is posix.minor
    for device in (os.stat(".").st_dev, os.stat("/dev/null").st_rdev):
        assert os.makedev(posix.major(device), posix.minor(device)) == device

    high = posix.makedev(1 << 31, 0x12345678)
    assert high >= 1 << 63
    assert (os.major(high), os.minor(high)) == (1 << 31, 0x12345678)
    try:
        posix.makedev(1 << 32, 0)
    except OverflowError:
        pass
    else:
        raise AssertionError("accepted a major number wider than an unsigned int")
