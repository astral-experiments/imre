import stat
from stat import S_IMODE as permissions


def flag() -> bool:
    print("flag")
    return True


print(permissions(flag()))
print(stat.filemode(False))
print(stat.S_ISDIR(False))
print(permissions(*(True,)))
classify = stat.S_ISDIR
print(classify(stat.S_IFDIR | stat.S_IRUSR))
for mode in (-1, 18446744073709551616, 1267650600228229401496703205376):
    try:
        print(stat.filemode(mode))
    except OverflowError as exc:
        print(str(exc))
