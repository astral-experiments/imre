import codecs
import sys
from sys import getdefaultencoding, getfilesystemencodeerrors, getfilesystemencoding


def show():
    # argv supplies a distinct live string, even if the spelling is interned.
    text = sys.argv[1]
    interned = sys.intern(text)
    unicode = getdefaultencoding()
    filesystem = getfilesystemencoding()
    errors = getfilesystemencodeerrors()
    print(unicode)
    print(filesystem)
    print(errors)
    print(unicode is sys.getdefaultencoding())
    print(filesystem is sys.getfilesystemencoding())
    print(errors is sys.getfilesystemencodeerrors())
    print(unicode is filesystem)
    print(sys._is_interned(unicode))
    print(sys._is_interned(filesystem))
    print(sys._is_interned(errors))
    print(text is interned)
    print(interned is unicode)
    print(interned is filesystem)
    print(interned is errors)
    print(unicode is codecs.lookup(unicode).name)
    print(filesystem is codecs.lookup(filesystem).name)
    utf8 = "utf-8"
    ascii_name = "ascii"
    handler = "surrogateescape"
    print(unicode is utf8)
    print(filesystem is ascii_name)
    print(errors is handler)


def argument(value: int) -> int:
    print('argument')
    print(value)
    return value


show()
try:
    getfilesystemencodeerrors(argument(1), argument(2))
except TypeError as err:
    print(str(err))
try:
    sys.getfilesystemencoding(unexpected=argument(3))
except TypeError as err:
    print(str(err))
try:
    getdefaultencoding(argument(4))
except TypeError as err:
    print(str(err))
