import os
from pathlib import Path, PureWindowsPath
from typing import Callable


class StaticName:
    @staticmethod
    def __fspath__() -> str:
        return 'static'


class ClassName:
    @classmethod
    def __fspath__(cls) -> str:
        return 'class'


class DefaultName:
    def __fspath__(self, value: str = 'positional', *, suffix: str = 'keyword') -> str:
        return value + suffix


class Missing:
    def __fspath__(self, required: str) -> str:
        return required


class Child(Missing):
    pass


class TextProperty:
    @property
    def __fspath__(self) -> str:
        print('getting text')
        return 'not callable'


class FailedLookup:
    @property
    def __fspath__(self) -> str:
        print('looking up a broken descriptor')
        raise ValueError('descriptor lookup')


class MethodProperty:
    def spell(self) -> str:
        print('calling the bound method')
        return 'from property'

    @property
    def __fspath__(self) -> Callable[[], str]:
        print('getting the bound method')
        return self.spell


class FailedCall:
    def spell(self) -> str:
        raise ValueError('descriptor call')

    @property
    def __fspath__(self) -> Callable[[], str]:
        return self.spell


print(str(Path(StaticName(), ClassName(), DefaultName())))
print(str(PureWindowsPath('root').joinpath(MethodProperty())))
try:
    os.fspath(Child())
except TypeError as error:
    print(str(error))
try:
    Path(Child())
except TypeError as error:
    print(str(error))
try:
    os.fspath(TextProperty())
except TypeError as error:
    print(str(error))
try:
    os.fspath(FailedLookup())
except TypeError as error:
    print(str(error))
try:
    Path(FailedLookup())
except TypeError as error:
    print(str(error))
try:
    PureWindowsPath(FailedCall())
except ValueError as error:
    print(str(error))
