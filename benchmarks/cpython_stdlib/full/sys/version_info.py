"""Cross-check the published language release without assuming the implementation's patch level."""
import sys
from sys import version_info as imported
from version_info_owner import is_shared_version

def other() -> bool:
    return imported is sys.version_info

def check() -> None:
    info = sys.version_info
    assert other()
    assert is_shared_version(imported)
    assert info is sys.version_info
    assert info is not info[:]
    assert sys.version.startswith(str(info.major) + "." + str(info.minor) + "." + str(info.micro))
    assert info[:2] == (info.major, info.minor)
    assert info[2:] == (info.micro, info.releaselevel, info.serial)
    assert info[::-1] == (info.serial, info.releaselevel, info.micro, info.minor, info.major)
    assert info.__match_args__ == ("major", "minor", "micro", "releaselevel", "serial")
    assert sys.hexversion == ((info.major << 24) | (info.minor << 16) | (info.micro << 8) | 0xf0 | info.serial)
    assert repr(info) == ("sys.version_info(major=" + str(info.major) + ", minor=" + str(info.minor)
                          + ", micro=" + str(info.micro) + ", releaselevel=" + repr(info.releaselevel)
                          + ", serial=" + str(info.serial) + ")")

check()
