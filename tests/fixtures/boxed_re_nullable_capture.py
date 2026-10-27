import re


def current():
    for pattern, text in (
        ('((a?)*)', 'a'),
        ('(a?)*a', 'aa'),
        ('(a?)+', 'aa'),
        ('(a*)*', 'aa'),
        ('(a*)+', 'aa'),
        ('(?P<item>a?)*', 'a'),
        ('([ab]?)*b', 'aab'),
        ('(é?)*', 'é東'),
        (b'(a?)*', b'aa'),
    ):
        compiled = re.compile(pattern)
        matched = compiled.match(text)
        print(None if matched is None else matched.groups())
        print(compiled.split(text, 1))
