import os
from pathlib import Path, PurePath, PurePosixPath, PureWindowsPath


def show(p: PureWindowsPath) -> None:
    print(repr(p))
    print(os.fspath(p))
    print(p.as_posix())
    print(p.drive)
    print(p.root)
    print(p.anchor)
    print(p.name)
    print(p.stem)
    print(p.suffix)
    print(str(p.parent))
    print(p.is_absolute())


for parent in ['', '/', '//', '///', 'C:', 'C:/', 'a/b', r'\\host',
               '//host/share', '//host/share/spam', '//?/UNC/server/share',
               '//./device', '//?./share', '//.?/share', '/?/UNC', './c:s', 'é:/', 'Σ:/', '/.']:
    for child in ['', '.', '..', 'a.txt', './c:s', 'C:', 'c:leaf', '/tail',
                  'D:leaf', 'd:/root', '//other/share', 'a///b', '//?/C:']:
        p = PureWindowsPath(parent, child)
        show(p)
        print(p == PureWindowsPath(parent) / child)
        show(PureWindowsPath('C:/root', p))
        print(str(PurePosixPath(p)))
        print(str(Path(p)))

for share in [r'\\server\share', '//server/share', '//SERVER/SHARE']:
    print(PureWindowsPath('//server/share/dir', share).as_posix())

print(PurePath('a/b') == Path('a', 'b'))
print(PureWindowsPath('a/b') == PurePosixPath('a/b'))
print(repr(PurePath('//')))
print(repr(PurePosixPath('//a') / PureWindowsPath('c:/tmp')))
keys = {PureWindowsPath('C:/A'): 'found'}
print(keys[PureWindowsPath('c:/a')])

for spelling in ["a/b", "c:/a/b", "//share/path/a", "a\\b"]:
    for pattern in ["a/b", "a\\b", "**/b", "*/b", "c:/*/b", "//share/*/*"]:
        w, u = PureWindowsPath(spelling), PurePosixPath(spelling)
        wp, up = PureWindowsPath(pattern), PurePosixPath(pattern)
        print(w.match(up))
        print(w.full_match(up))
        print(u.match(wp))
        print(u.full_match(wp))
