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
