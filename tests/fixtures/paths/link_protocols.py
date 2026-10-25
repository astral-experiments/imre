from enum import StrEnum
from pathlib import Path


class Name:
    value: str | bytes

    def __init__(self, value: str | bytes) -> None:
        self.value = value

    def __fspath__(self) -> str | bytes:
        print('convert name')
        return self.value


class Bad:
    def __fspath__(self) -> int:
        print('bad protocol')
        return 7


class Missing:
    pass


class Target(StrEnum):
    FILE = './real'


class EnumName:
    def __fspath__(self) -> Target:
        return Target.FILE


class Flag:
    def __bool__(self) -> bool:
        print('convert flag')
        return True


def flag() -> Flag:
    print('evaluate flag')
    return Flag()


def argument(value: str | bytes) -> Name:
    print('evaluate name')
    return Name(value)


p = Path('protocol-link')
p.symlink_to(argument('./absent'), flag())
print(str(p.readlink()))
try:
    p.symlink_to(target_is_directory=flag(), target=argument(b'./other'))
except FileExistsError as error:
    print(repr(error.filename))
    print(repr(error.filename2))
try:
    Path('bad\0destination').symlink_to(argument(b'valid'), flag())
except ValueError as error:
    print(str(error))
try:
    Path('bad\0destination').symlink_to(argument(b'bad\0source'), flag())
except ValueError as error:
    print(str(error))
try:
    Path('bad\0destination').symlink_to(Bad(), flag())
except TypeError as error:
    print(str(error))
try:
    Path('bad\0destination').symlink_to(Missing(), flag())
except TypeError as error:
    print(str(error))
try:
    Path('hard').hardlink_to(Bad())
except TypeError as error:
    print(str(error))
try:
    Path('hard').hardlink_to(Missing())
except TypeError as error:
    print(str(error))
Path('real').write_bytes(b'linked')
Path('alias').hardlink_to(argument(b'./real'))
print(Path('alias').read_bytes())
Path('enum-link').symlink_to(Target.FILE)
print(str(Path('enum-link').readlink()))
Path('enum-hard').hardlink_to(EnumName())
print(Path('enum-hard').read_bytes())
