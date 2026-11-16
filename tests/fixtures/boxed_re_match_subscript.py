import re


class Index:
    def __index__(self):
        return 1


class Raises:
    def __index__(self):
        raise ValueError("failed index")


def current():
    class Meta(type):
        pass

    class Subject(metaclass=Meta):
        pass

    for subject, name in (
        (type, "type"),
        (int, "type"),
        (len, "builtin_function_or_method"),
        (Subject, "Meta"),
        (Subject(), "Subject"),
    ):
        try:
            re.search("x*", subject)
        except TypeError as error:
            assert str(error) == "expected string or bytes-like object, got '%s'" % name
        else:
            assert False
    for pattern, subject, want in (
        (r"(?P<outer>(?P<inner>))", "", (1, "outer")),
        (r"(?P<first>)(?P<second>)", "", (2, "second")),
        (r"(?=a(?P<look>bc))(?P<consume>ab)", "abc", (2, "consume")),
        (r"(?:(?P<a>a)|(?P<b>b))+", "ba", (1, "a")),
        (r"(?:(?P<empty>\Z)|(?P<char>.))+", "x", (1, "empty")),
    ):
        match = re.match(pattern, subject)
        assert (match.lastindex, match.lastgroup) == want
    assert re.search(r"(.*?b|.*?a)(.*)", "ab").groups() == ("ab", "")
    match = re.search(r"(?P<left>.*?b|.*?a)(?P<tail>.*)(?:(?P<t>x)?)*", "ab")
    assert (match.groups(), match.lastgroup) == (("ab", "", None), "tail")
    data = bytearray(b"!abz")
    match = re.compile(br"(?P<outer>(a))(b)$").search(data, 1, 3)
    data[:] = b""
    assert (match.pos, match.endpos) == (1, 3)
    assert match.regs == ((1, 3), (1, 2), (1, 2), (2, 3))
    assert match.regs is match.regs
    assert (match.lastindex, match.lastgroup) == (3, None)
    for spelling in (r"(?P<a>a)|(?P<b>b)", r"(?=(?P<a>a)|(?P<b>b))[ab]"):
        pattern = re.compile(spelling)
        first = pattern.search("!a")
        assert pattern.search("!") is None
        second = pattern.match("b")
        assert pattern.fullmatch("b").groups() == (None, "b")
        assert pattern.fullmatch("aa") is None
        assert pattern.fullmatch("a").groups() == ("a", None)
        assert (second.groups(), second.lastgroup) == ((None, "b"), "b")
        assert (first.groups(), first.lastgroup) == (("a", None), "a")
        assert first.regs == ((1, 2), (1, 2), (-1, -1))
    parts = re.compile(r"(é)|(a)").finditer("xéa!", 1, 3)
    first, second = tuple(parts)
    assert first.regs == ((1, 2), (1, 2), (-1, -1))
    assert (second.pos, second.endpos, second.regs) == (
        1, 3, ((2, 3), (-1, -1), (2, 3)),
    )
    for pattern, text in [(r"(?P<word>é)(b)?", "é"), (br"(?P<word>\xff)(b)?", b"\xff")]:
        match = re.compile(pattern).match(text)
        assert match[0] == text
        assert match[1] == match["word"] == match[Index()] == text
        assert match[2] is None
        for key in (-1, 3, 1 << 100, "missing", b"word", (1, 2), slice(0, 1)):
            try:
                match[key]
            except IndexError as error:
                assert str(error) == "no such group"
            else:
                raise AssertionError("invalid match group accepted")
        try:
            match[Raises()]
        except ValueError as error:
            assert str(error) == "failed index"
        else:
            raise AssertionError("index exception was lost")
    match = re.match(r"(?P<word>ab)(?P<tail>x)?", "ab")
    assert "{word}:{tail}".format_map(match) == "ab:None"
    try:
        "{missing}".format_map(match)
    except IndexError as error:
        assert str(error) == "no such group"
    else:
        raise AssertionError("mapping lookup error was lost")
    assert "{v:{spec}}".format_map({"v": 15, "spec": "x"}) == "f"
    for template in ("{}", "{0}", "{v:{0}}"):
        try:
            template.format_map({"v": "x"})
        except ValueError as error:
            assert str(error) == "Format string contains positional fields"
        else:
            raise AssertionError("positional field accepted")
    subject = chr(0x1f40d) + chr(0xd800) + chr(0xe000) + "Z"
    pattern = re.compile(r"(?P<value>.)(.)(a)?")
    match = pattern.fullmatch(subject, pos=1, endpos=3)
    assert match.string is subject and match.span("value") == (1, 2)
    assert match.groups() == (chr(0xd800), chr(0xe000), None)
    assert pattern.search(subject, 3, 1) is None
    assert re.match(r"..", subject).span() == (0, 2)
    assert re.fullmatch(pattern=r".+z", string=subject, flags=re.I)[0] == subject
    assert repr(match) == r"<re.Match object; span=(1, 3), match='\ud800\ue000'>"
    long_match = re.search("é+'", "🐍" + "é" * 50 + "'")
    assert repr(long_match) == '<re.Match object; span=(1, 52), match="' + "é" * 49 + ">"
    data = bytearray(b"abcd")
    retained = re.search(b"bc", data)
    data[:] = b"aX"
    expected = "<re.Match object; span=(1, 3), match=b'X'>"
    assert str(retained) == repr(retained) == expected
    assert repr([retained]) == "[" + expected + "]"
    view = memoryview(b"abc")
    retained = re.search(b"abc", view)
    view.release()
    try:
        repr(retained)
    except TypeError as error:
        assert str(error) == "expected string or bytes-like object, got 'memoryview'"
    else:
        raise AssertionError("released subject accepted")
    high, low, private = chr(0xd800), chr(0xdfff), chr(0xf0000)
    source = "(?P<odd>" + high + ")(" + low + ")"
    pattern = re.compile(source, re.I)
    subject = private + low + high + low
    match = pattern.search(subject)
    assert pattern.pattern is source and match.string is subject
    assert match.span() == (2, 4) and match["odd"] == high and match[2] == low
    flagged = re.compile("(?im)(?P<letter>a)" + high)
    assert flagged.flags == re.I | re.M | re.U
    assert flagged.fullmatch("A" + high).lastgroup == "letter"
    assert pattern.search(private + low) is None
    assert re.fullmatch(high + "|x", private) is None
    assert re.fullmatch(high + "|x", "x")[0] == "x"
    for bad, pos, message in [
        (high + "(", 1, "missing ), unterminated subpattern"),
        ("(?P<foo" + high + ">x)", 4, "bad character in group name " + repr("foo" + high)),
    ]:
        try:
            re.compile(bad)
        except re.PatternError as error:
            assert error.pattern is bad and error.pos == pos and error.msg == message
        else:
            raise AssertionError("invalid pattern accepted")
    print("text and bytes match groups passed")
