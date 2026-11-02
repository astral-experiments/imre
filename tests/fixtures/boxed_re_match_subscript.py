import re


class Index:
    def __index__(self):
        return 1


class Raises:
    def __index__(self):
        raise ValueError("failed index")


def current():
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
    print("text and bytes match groups passed")
