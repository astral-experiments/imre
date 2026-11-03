import re


def current():
    bounded = re.compile("(é)|(.)")
    assert bounded.findall(string="xé中y", pos=1, endpos=3) == [("é", ""), ("", "中")]
    assert re.compile("^x|é$").findall("xxé!", 1, 3) == ["é"]
    assert re.compile(rb".*?").findall(b"xyz", 1, 2) == [b"", b"y", b""]
    assert re.compile("").findall("ab", 2, 1) == []

    text = "é\udcff\ue000\ud800"
    assert re.compile(r"|.").findall(text, 1, 3) == ["", "\udcff", "", "\ue000", ""]
    assert re.findall(r"(a)?(.)", text) == [("", char) for char in text]
    assert re.findall("[\udcff\ud800]", text + "\U000f0000") == ["\udcff", "\ud800"]

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
    class Exporter(bytes):
        def __buffer__(self, flags):
            return memoryview(b"override")

    source = Exporter(b"abc")
    assert bytes(source) == b"abc"
    for acquire in (lambda: bytes(memoryview(source)), lambda: re.findall(b".+", source)[0]):
        try:
            result = acquire()
        except NotImplementedError as error:
            assert "buffer hooks" in str(error)
        else:
            assert result == b"override", result
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
