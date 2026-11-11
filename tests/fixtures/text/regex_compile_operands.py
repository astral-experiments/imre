import re


def pattern() -> str:
    print('pattern')
    return '^From '


def flags() -> int:
    print('flags')
    return 8


def subject() -> str:
    print('subject')
    return 'line\nFrom here'


print(re.compile(pattern(), flags()).search(subject()) is not None)
