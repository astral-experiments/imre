from enum import StrEnum
from pathlib import Path, PurePosixPath, PureWindowsPath


class TextName:
    name: str

    def __init__(self, name: str) -> None:
        self.name = name

    def __fspath__(self) -> str:
        print(f'convert {self.name}')
        return self.name


class ByteName:
    def __fspath__(self) -> bytes:
        print('convert bytes')
        return b'file'


class WrongReturn:
    def __fspath__(self) -> int:
        print('convert int')
        return 4


class TypeFailure:
    def __fspath__(self) -> str:
        print('raise TypeError')
        raise TypeError('inside __fspath__')


class ValueFailure:
    def __fspath__(self) -> str:
        print('raise ValueError')
        raise ValueError('inside __fspath__')


class NoProtocol:
    pass


class Segment(StrEnum):
    FIXTURE = 'a/b'


class EnumName:
    def __fspath__(self) -> Segment:
        return Segment.FIXTURE


class Either:
    binary: bool

    def __init__(self, binary: bool) -> None:
        self.binary = binary

    def __fspath__(self) -> str | bytes:
        if self.binary:
            return b'bytes'
        return 'string'


def make(name: str) -> TextName:
    print(f'evaluate {name}')
    return TextName(name)


def later() -> str:
    print('evaluate later')
    raise ValueError('later argument expression')


def receiver() -> PureWindowsPath:
    print('evaluate receiver')
    return PureWindowsPath('prefix')


class InheritedName(TextName):
    pass


class UpperName(TextName):
    def __fspath__(self) -> str:
        print(f'uppercase {self.name}')
        return self.name.upper()


def relative_name(name: TextName) -> None:
    print(PurePosixPath('a/b').is_relative_to(name))
    print(str(PureWindowsPath('a/b').relative_to(name)))


def walk_option(fail: bool) -> bool:
    print('evaluate walk_up')
    if fail:
        raise ValueError('walk_up failed')
    return True


print(str(Path(make('first'), make('second'))))
print(str(receiver().joinpath(make('one'), make('two'))))
print(str(receiver().with_segments(make('fresh'), make('part'))))
print(str(PureWindowsPath('base').joinpath()))
print(str(PureWindowsPath('base').with_segments()))
print(str(PurePosixPath(PureWindowsPath('a/b'), TextName('c'))))
print(str(Path(Either(False))))
print(str(Path(Segment.FIXTURE)))
print(str(PureWindowsPath(EnumName())))
try:
    Path(Either(True))
except TypeError as exc:
    print(str(exc))
try:
    Path(1)
except TypeError as exc:
    print(str(exc))
try:
    Path(b'file')
except TypeError as exc:
    print(str(exc))
try:
    PureWindowsPath('prefix').joinpath('b', 1)
except TypeError as exc:
    print(str(exc))
try:
    Path(ByteName())
except TypeError as exc:
    print(str(exc))
try:
    PureWindowsPath(WrongReturn())
except TypeError as exc:
    print(str(exc))
try:
    Path('a').with_segments(TypeFailure())
except TypeError as exc:
    print(str(exc))
try:
    PurePosixPath(ValueFailure())
except ValueError as exc:
    print(str(exc))
try:
    Path(NoProtocol(), later())
except ValueError as exc:
    print(str(exc))
try:
    receiver().joinpath(TypeFailure(), later())
except ValueError as exc:
    print(str(exc))
try:
    receiver().with_segments(NoProtocol(), make('evaluated before the error'))
except TypeError as exc:
    print(str(exc))

try:
    Path('a').joinpath('b', 1)
except TypeError as exc:
    print(str(exc))
try:
    Path(path='file')
except TypeError as exc:
    print(str(exc))
try:
    Path('a').joinpath(child='b')
except TypeError as exc:
    print(str(exc))
try:
    receiver().with_segments(make('evaluated'), self=make('also evaluated'), bad=3)
except TypeError as exc:
    print(str(exc))
try:
    PureWindowsPath(NoProtocol(), bad=later())
except ValueError as exc:
    print(str(exc))
try:
    PurePosixPath(bad=make('first kw'), self=make('second kw'))
except TypeError as exc:
    print(str(exc))


relative_name(InheritedName('a'))
relative_name(UpperName('a'))
print(str(receiver().relative_to(make('base'), walk_up=walk_option(False))))
try:
    receiver().relative_to(TypeFailure(), walk_up=walk_option(True))
except ValueError as exc:
    print(str(exc))
try:
    PureWindowsPath('a').is_relative_to(ByteName())
except TypeError as exc:
    print(str(exc))
try:
    PurePosixPath('a').relative_to(WrongReturn())
except TypeError as exc:
    print(str(exc))
try:
    Path('a').is_relative_to(TypeFailure())
except TypeError as exc:
    print(str(exc))
try:
    PureWindowsPath('a').relative_to(ValueFailure())
except ValueError as exc:
    print(str(exc))
