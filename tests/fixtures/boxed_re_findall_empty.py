import re


def current():
    for pattern, text in (
        ('a*', 'a'),
        ('a*?', 'a'),
        ('|a', 'a'),
        ('(a*)', 'a'),
        ('(a)|(b)', 'xab'),
        ('(?:|é)', 'é東'),
        ('$', 'a\n'),
        (b'a*?', b'\xffa'),
    ):
        print(re.compile(pattern).findall(text))
    for source, text in [(r'(a)(?P<tail>b){0}', 'a'), (b'(a)(?P<tail>b){0,0}?', b'a')]:
        for flags in (0, re.VERBOSE):
            pattern = re.compile(source, flags)
            match = pattern.fullmatch(text)
            print(pattern.groups, match.groups('missing'), match.group('tail'))
            print(pattern.findall(text), pattern.split(text))
