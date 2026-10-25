from os import stat_result
from pathlib import Path
from typing import Callable


class Field:
    stat: int

    def __init__(self) -> None:
        self.stat = 3


class WrongArguments:
    def stat(self, required: str) -> stat_result:
        raise AssertionError('binding must fail before entering stat')


class Getter:
    @property
    def stat(self) -> int:
        print('noncallable getter')
        return 1


class MissingGetter:
    @property
    def stat(self) -> int:
        print('missing getter')
        raise AttributeError('use the path')

    def __fspath__(self) -> str:
        print('convert after the getter')
        return 'input.txt'


class Static:
    @staticmethod
    def stat() -> stat_result:
        print('static stat')
        return Path('input.txt').stat()


class ClassBound:
    @classmethod
    def stat(cls) -> stat_result:
        print('class stat')
        return Path('input.txt').stat()


class BoundGetter:
    def read(self) -> stat_result:
        print('called bound method')
        return Path('input.txt').stat()

    @property
    def stat(self) -> Callable[[], stat_result]:
        print('looked up bound method')
        return self.read


class TextInode:
    @property
    def st_ino(self) -> str:
        print('text inode')
        return 'not an integer'

    @property
    def st_dev(self) -> int:
        raise AssertionError('device read after unequal inode')


class Other:
    def stat(self) -> TextInode:
        return TextInode()


p = Path('input.txt')
try:
    p.samefile(Field())
except TypeError as exc:
    print(str(exc))
try:
    p.samefile(WrongArguments())
except TypeError as exc:
    print(str(exc))
try:
    p.samefile(Getter())
except TypeError as exc:
    print(str(exc))
print(p.samefile(MissingGetter()))
print(p.samefile(Static()))
print(p.samefile(ClassBound()))
print(p.samefile(BoundGetter()))
print(p.samefile(Other()))
