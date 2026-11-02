import re


def current():
    # A successful atomic choice cannot be retried to satisfy its suffix.
    for expr, text, expected in (
        (r"(a|ab)c", "abc", ("ab",)), (r"(?>(a|ab))c", "abc", None),
        (r"(a*+)(b)", "aaab", ("aaa", "b")), (r"a*+a", "aaa", None),
        (r"a{1,2}+a", "aa", None), (r"a{1,2}a", "aa", ()),
        (r"(?>a?b){2}", "abb", ()), (r"(?:.(?!D))++C", "ABC", None),
        (br"(?>(a+))b", b"aab", (b"aa",)), (br".++", b"\xff\x80", ()),
        (r"[+?]++", "+?", ()), (r"\++a", "++a", ()), (r"\}+a", "}}a", ()),
        ("(?x:(?> a | ab ) c)", "abc", None),
    ):
        m = re.fullmatch(expr, text)
        assert (None if m is None else m.groups()) == expected, (expr, text)
    assert [m.span() for m in re.finditer(r"a?+", "a-")] == [(0, 1), (1, 1), (2, 2)]
    for bad in ("a*++", "a{1,2}+?", "(?>a)**", "(?x:(?>a)* +)"):
        try:
            re.compile(bad)
        except re.PatternError:
            pass
        else:
            raise AssertionError(bad)
    for expr, text, expected in (
        (r"(?P<outer>(a)|b)*+", "ab", (1, "outer")),
        (r"(?P<outer>(a)|b)*+", "a", (1, "outer")),
        (r"(?:(?P<x>x)|(?P<y>y))++", "xy", (2, "y")),
        (r"(?:(?P<x>x)|(?P<y>y))++", "yx", (1, "x")),
        (r"(?:(?>(a|ab))c|a(b)c)", "abc", (2, None)),
        (r"(?:(?P<a>a)|(?P<b>)b)++", "ab", (2, "b")),
        (r"(?>(a))(b)", "ab", (2, None)),
    ):
        m = re.fullmatch(expr, text)
        assert (m.lastindex, m.lastgroup) == expected, (expr, text)
    # Even with captured operands the committed choice cannot retreat for the suffix.
    assert re.fullmatch(r"(?P<outer>(a)|b)++a", "aba") is None
    for flags in (0, re.L):
        ahead = re.compile(b"a(?!b).", flags)
        assert ahead.search(b"abad").span() == (2, 4)
        assert ahead.fullmatch(b"ab") is None
        for pattern in (b"^([ab]*?)(?=(b)?)c", b"^([ab]*?)(?!(b))c"):
            match = re.fullmatch(pattern, b"abc", flags)
            assert match.groups() == (b"ab", None) and match.span(2) == (-1, -1)
        peek = re.compile(b"(?=(.)(.))", flags)
        assert peek.match(b"x\xff\x80", 1, 3).groups() == (b"\xff", b"\x80")
        assert peek.match(b"x\xff\x80", 1, 2) is None

        guard = re.compile(b"(?<!abc)(d.f)", flags)
        assert guard.search(b"abcdefdof", 3, 6) is None
        assert guard.search(b"xbcdefdof", 3, 6).span(1) == (3, 6)
        groups = re.compile(b"^([ab]*?)(?<!(?P<before>a))(?P<end>c)", flags)
        match = groups.fullmatch(b"abc")
        assert match.groups() == (b"ab", None, b"c") and match.span("before") == (-1, -1)
        assert groups.groupindex["end"] == 3 and groups.search(b"ac") is None
        high = re.compile(b"(?<=\xff\x80)(.)", flags)
        assert high.match(b"\xff\x80x", 2, 3).group(1) == b"x"
        assert high.match(b"\xff\x81x", 2, 3) is None

    probe = re.compile(r"(?=(?P<a>.)(?P<b>.))(?P<first>.)")
    found = probe.search("xé中!", 1, 3)
    assert re.match(r"(?=(?P<é>x))", "x").group("é") == "x"
    assert probe.groups == 3
    assert found.groups() == ("é", "中", "é")
    assert (found.span(), found.span("b")) == ((1, 2), (2, 3))
    assert probe.match("xé中!", 1, 2) is None
    assert probe.fullmatch("xé中!", 1, 3) is None
    assert re.fullmatch(r"(?=a|ab)ab", "ab").span() == (0, 2)

    choice = re.compile(r"(?:(?=(a))aX|(?P<other>a))")
    assert choice.fullmatch("a").groups() == (None, "a")
    assert re.fullmatch(r"(?!(a))(?P<b>b)", "b").groups() == (None, "b")
    assert re.match(r"(?=(a|ab))", "ab").groups() == ("a",)
    assert re.findall(r"(?=(.))|.", "éβ") == ["é", "", "β", ""]
    assert [m.span() for m in re.finditer(r"(?=$)", "x\n")] == [(1, 1), (2, 2)]
    assert re.split(r"(?=:)", "a:b::c") == ["a", ":b", ":", ":c"]
    assert re.compile(r"(?=a)|a").sub("X", "a") == "XX"

    number = re.compile(r"(?=\d|\.\d) (?P<whole>\d*) (?: \. (?P<frac>\d*) )?", re.X)
    assert number.fullmatch(".٣").groups() == ("", "٣")
    assert number.fullmatch(".") is None
    assert re.fullmatch(r"(?=[i])i", "ı", re.I)
    assert re.fullmatch(r"(?=[i])i", "ı", re.I | re.A) is None
    assert re.match(r"(?=a.)(a.)", "a\n", re.S).group(1) == "a\n"
    assert re.search(r"^(?=.)a$", "b\na\nc", re.M).span() == (2, 3)
    for pattern in (r"(?=a)\K", r"(?=a)\G", r"(?=a)(?R)", r"(?=a)(?|x)", r"(?=a)(?P<n>x)(?P<n>y)"):
        try:
            re.compile(pattern)
        except re.PatternError:
            pass
        else:
            raise AssertionError(pattern)

    assert re.fullmatch(r"(?=(a|ab))\1c", "abc") is None
    assert re.fullmatch(r"(?=(a+))\1", "aaa").groups() == ("aaa",)
    assert re.fullmatch(r"(?=a)(a|ab)\1c", "ababc").groups() == ("ab",)
    assert re.fullmatch(r"(?=(?P<π>a))(?P=π)(b)\2\1", "abba").groups() == ("a", "b")
    assert re.fullmatch(r"(?=(?P<g>a)?)(?(g)a|b)", "b").groups() == (None,)
    assert re.fullmatch(r"(?=(a?))(?(1)b|c)", "b").groups() == ("",)
    assert re.fullmatch(r"(a)b(?=(?(2)x|c))(c)", "abc").groups() == ("a", "c")
    assert re.fullmatch(r"(?=b)(?:(?(1)a|b)(a)?)+", "baaa").group(1) == "a"
    assert re.match(r"(?=(?(2)(a)|(b)))(c)?", "b").groups() == (None, "b", None)
    for invalid in (r"(?=a)(a\1)", r"(?=a)\1(a)", r"(?=a)(?(2)x|a)(a)",
                    r"(?=a)(?P=x)(?P<x>a)", r"(?=a)(a)(?(1)a|b|c)"):
        try:
            re.compile(invalid)
        except re.PatternError:
            pass
        else:
            raise AssertionError(invalid)

    # Python classifies marks, connector punctuation other than _, and join controls as non-word.
    boundary = re.compile(r"\b(?=(.))")
    assert [(m.start(), m.group(1)) for m in boundary.finditer("é\u0301中\u203f_")] == [
        (0, "é"), (1, "\u0301"), (2, "中"), (3, "\u203f"), (4, "_")]
    assert boundary.search("a中", 1) is None
    assert re.fullmatch(r"\B(?=$)", "").span() == (0, 0)
    assert [m.start() for m in re.finditer(r"\B(?=.)", "a_\u200d!")] == [1, 3]

    # Lookbehind can read and capture before pos. The assertion consumes no input.
    p = re.compile(r"(?<=\A(?P<prefix>é|𐀀))(b)")
    m = p.fullmatch("𐀀b!", 1, 2)
    assert m.groups() == ("𐀀", "b") and m.span() == (1, 2) and m.span("prefix") == (0, 1)
    assert p.search("xéb", 2) is None
    assert re.fullmatch(r"ab(?<=b)cd", "abcd").span() == (0, 4)
    assert re.fullmatch(r"ab(?<!b)cd", "abcd") is None
    assert re.search(r"(?<!x{2})z", "z").span() == (0, 1)
    assert re.search(r"(ab)\1(?<=\1)c", "ababc").groups() == ("ab",)
    assert re.match(r"(?:(a)|(x))b(?<=(?(2)x|b))c", "abc").groups() == ("a", None)
    assert re.search(r"(?<=(?:a{2}|bb))c", "aac").span() == (2, 3)
    assert re.search(r"(?<!(?:a{2}|bb))c", "bbc") is None
    assert re.search(r"(?<!(?:a{2}|bb))c", "abc").span() == (2, 3)
    assert re.match(r"(a{2}|bb)c(?<=\1c)", "aac").groups() == ("aa",)
    for invalid in (r"(?<=x*)z", r"(?<!é|ab)z", r"(?<=a{1,2})", r"(a+)(?<=\1)",
                    r"(?<=(a)\1)", r"(?<=(?(1)x|y))(a)"):
        try:
            re.compile(invalid)
        except re.PatternError:
            pass
        else:
            raise AssertionError(invalid)
