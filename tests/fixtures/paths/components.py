from pathlib import Path, PurePosixPath, PureWindowsPath


def windows(p: PureWindowsPath, other: PureWindowsPath, walk: bool) -> None:
    print(p.is_relative_to(other))
    try:
        print(str(p.relative_to(other, walk_up=walk)))
    except ValueError as error:
        print(str(error))


def posix(p: Path, other: Path, walk: bool) -> None:
    print(p.is_relative_to(other))
    try:
        print(str(p.relative_to(other, walk_up=walk)))
    except ValueError as error:
        print(str(error))


def failing_option() -> bool:
    raise ValueError('evaluated walk_up')


for spelling in ["", "/", "//", "a/b", "a/../b", "a/.", "/a", "//a", "x/../a"]:
    for base in ["", "/", "//", "a", "a/b", "x/..", "/x/..", "a/../b", "a/../x"]:
        for walk in [False, True]:
            posix(Path(spelling), Path(base), walk)
for spelling in ["", "/", "C:", "C:/A", "C:a", "D:/a", "//Srv/Share/Dir/a", "x/../a"]:
    for base in ["/", "c:", "C:/a", "C:A/B", "D:/a", "//sRv/sHare", "C:/x/..", "x/../b"]:
        for walk in [False, True]:
            windows(PureWindowsPath(spelling), PureWindowsPath(base), walk)
print(PurePosixPath("a").is_relative_to(PureWindowsPath("a")))
print(PureWindowsPath("a").is_relative_to(PurePosixPath("a")))
try:
    print(str(PureWindowsPath("a").relative_to(PurePosixPath(".."), walk_up=True)))
except ValueError as error:
    print(str(error))

for spelling in ["", "/", "a/b", "x/a..", ".a.b.", "..", "//server/share"]:
    p = PureWindowsPath(spelling)
    print("|".join(p.suffixes))
    for value in ["", ".", "new", ".gz", "x/y", ".x/.y"]:
        try:
            print(str(p.with_stem(value)))
        except ValueError as error:
            print(str(error))
        try:
            print(str(p.with_suffix(value)))
        except ValueError as error:
            print(str(error))


p = PureWindowsPath('a.tar.gz')
extensions = p.suffixes
extensions.append('.changed')
print('|'.join(extensions))
print('|'.join(p.suffixes))
try:
    print(str(p.relative_to()))
except TypeError as error:
    print(str(error))
try:
    print(p.is_relative_to(b'a'))
except TypeError as error:
    print(str(error))
try:
    print(str(p.relative_to(None)))
except TypeError as error:
    print(str(error))
try:
    print(str(p.relative_to(b'a', walk_up=failing_option())))
except ValueError as error:
    print(str(error))
