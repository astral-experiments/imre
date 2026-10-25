from os import stat_result
from pathlib import Path, PureWindowsPath


class Name:
    spelling: str

    def __init__(self, spelling: str) -> None:
        self.spelling = spelling

    def __fspath__(self) -> str:
        print('fspath')
        return self.spelling


class Metadata:
    spelling: str

    def __init__(self, spelling: str) -> None:
        self.spelling = spelling

    def stat(self, follow: bool = True) -> stat_result:
        print(f'stat {follow}')
        return Path(self.spelling).stat()

    def __fspath__(self) -> str:
        raise AssertionError('samefile ignored stat')


class StatMissing:
    def stat(self) -> stat_result:
        print('stat attrerror')
        raise AttributeError('inside stat')

    def __fspath__(self) -> str:
        print('fallback')
        return 'input.txt'


class StatFailed:
    def stat(self) -> stat_result:
        print('failed stat')
        raise ValueError('inside stat')


class DifferentInode:
    st_ino: int

    def __init__(self) -> None:
        self.st_ino = -1

    @property
    def st_dev(self) -> int:
        raise AssertionError('device must not be read')


class ShortCircuit:
    def stat(self) -> DifferentInode:
        return DifferentInode()


class ResultMissing:
    def stat(self) -> int:
        return 3

    def __fspath__(self) -> str:
        raise AssertionError('comparison was inside the stat handler')


def argument() -> Name:
    print('evaluate operand')
    return Name('input.txt')


p = Path('input.txt')
alias = Path('inode-hard')
alias.hardlink_to(p)
print(p.samefile(p))
print(p.samefile('input.txt'))
print(p.samefile(other_path=alias))
print(p.samefile(Path('file-link')))
print(p.samefile(PureWindowsPath('file-link')))
print(p.samefile('command'))
print(p.samefile(argument()))
print(p.samefile(Metadata('input.txt')))
print(p.samefile(StatMissing()))
print(p.samefile(ShortCircuit()))
try:
    p.samefile(ResultMissing())
except AttributeError as exc:
    print(str(exc))
try:
    p.samefile(StatFailed())
except ValueError as exc:
    print(str(exc))
try:
    p.samefile(b'input.txt')
except TypeError as exc:
    print(str(exc))
try:
    p.samefile('missing')
except FileNotFoundError as exc:
    print(repr(exc.filename))


def bad_argument() -> str:
    print('evaluating later keyword')
    raise ValueError('argument before binding')


try:
    Path('missing-before-bind').samefile()
except TypeError as exc:
    print(str(exc))
try:
    p.samefile(argument(), 'extra')
except TypeError as exc:
    print(str(exc))
try:
    p.samefile(argument(), other_path=argument())
except TypeError as exc:
    print(str(exc))
try:
    p.samefile('input.txt', self=argument())
except TypeError as exc:
    print(str(exc))
try:
    p.samefile('input.txt', 'extra', unknown=argument())
except TypeError as exc:
    print(str(exc))
try:
    p.samefile('input.txt', 'extra', unknown=bad_argument())
except ValueError as exc:
    print(str(exc))
try:
    Path('missing-before-conversion').samefile(argument())
except FileNotFoundError as exc:
    print(repr(exc.filename))
try:
    Path('missing-before-stat').samefile(Metadata('input.txt'))
except FileNotFoundError as exc:
    print(repr(exc.filename))
