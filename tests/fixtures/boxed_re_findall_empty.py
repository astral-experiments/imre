import re


def current():
    source = b":x::"
    view = memoryview(source)
    other = memoryview(view)
    assert type(view) is memoryview and other.obj is source
    assert other is not view and other.readonly and len(other) == 4
    assert view == source == other and hash(view) == hash(source)
    assert view != source[1:] and re.findall(b":+", view) == [b":", b"::"]
    match = re.search(b":+", view)
    memoryview.release(view)
    assert match.span() == (0, 1) and bytes(other) == source
    try:
        match.group()
    except TypeError:
        pass
    else:
        raise AssertionError("released subject remained readable")
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
        compiled = re.compile(pattern)
        expected = compiled.findall(text)
        assert re.findall(pattern=pattern, string=text) == re.findall(compiled, text) == expected
        print(expected)
    assert re.findall(r"(a)|(b)", "Ab!", flags=re.I) == [("A", ""), ("", "b")]
    for source, text in [(r'(a)(?P<tail>b){0}', 'a'), (b'(a)(?P<tail>b){0,0}?', b'a')]:
        for flags in (0, re.VERBOSE):
            pattern = re.compile(source, flags)
            match = pattern.fullmatch(text)
            print(pattern.groups, match.groups('missing'), match.group('tail'))
            print(pattern.findall(text), pattern.split(text))
    for source, text in ((r"(?:a?|b)*", "ab"), (r"(?:a?|b){0,3}", "ba"),
                         (rb"((a?)*)", b"a"), (r"((a)?|b){1,3}", "b"),
                         (r"(?P<piece>a?|b)*?b", "ab"), (r"((a*)+){1,2}a", "aa")):
        pattern = re.compile(source)
        match = pattern.match(text)
        print(match.group(), match.groups(), pattern.fullmatch(text).groups())
        print(pattern.findall(text), pattern.split(text))
