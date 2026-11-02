import re


def current():
    from string import Template

    for flags in (re.I, re.I | re.X):
        pattern = re.compile(r"(?P<ascii>(?a:[a-z]))(?P<unicode>[a-z])", flags)
        assert pattern.fullmatch("KK").groups() == ("K", "K")
        for letter in "Kſİı":
            assert pattern.fullmatch(letter + "K") is None
        assert re.fullmatch(r"(?a:.[^a-z])", "éK", flags)
        assert re.fullmatch(r"(?a:\w(?u:\w)\w)\w", "AéZé", flags)
    assert re.fullmatch(r"(?a:(?=\b)\w)", "A")
    assert re.search(r"(?u:\b)", "\u0301") is None
    assert re.fullmatch(r"(?-i:I)\b", "İ", re.I) is None
    assert re.fullmatch("(?x:a # (?a: ignored\n)(?ui:b)", "aB")
    assert re.fullmatch("(?x:a # (?u: ignored\n)(?i-m:b)", "aB", re.A)
    assert re.fullmatch("(?x:a # (?-u: ignored\n)(?i-m:b)", "aB")
    assert re.fullmatch("(?x:\\b a # (?u: ignored\n)(?i-m:b)\\Z", "aB", re.A)
    assert re.fullmatch(r"(?ui:a)", "A")
    assert re.fullmatch(r"(?u-m:a)", "a")
    assert re.fullmatch(r"(?u:(?P<état>é))", "é")["état"] == "é"
    assert Template("$User ${Count} $$").substitute(User="Ann", Count=2) == "Ann 2 $"
    assert not Template("$K").is_valid()

    ascii_digits = re.compile(r"-?\d+", flags=re.ASCII)
    unicode_digits = re.compile(r"-?\d+")
    from re import Pattern
    assert type(ascii_digits) is Pattern is re.Pattern
    assert type(re.compile(b"x")) is Pattern
    assert isinstance(unicode_digits, Pattern)
    assert not isinstance(unicode_digits.match("1"), Pattern)
    assert type(Pattern) is type
    assert repr(Pattern) == "<class 're.Pattern'>"
    assert (Pattern.__module__, Pattern.__name__, Pattern.__qualname__) == ("re", "Pattern", "Pattern")
    assert Pattern[str].__origin__ is Pattern
    assert Pattern.__new__ is object.__new__
    for construct in (lambda: Pattern(), lambda: Pattern.__new__(Pattern)):
        try:
            construct()
        except TypeError as error:
            assert str(error) == "cannot create 're.Pattern' instances"
        else:
            raise AssertionError("constructed a pattern without compiling")
    from re import Match
    found = unicode_digits.match("1")
    assert type(found) is re.Match is Match
    assert type(re.search(b"x", b"xy")) is Match
    assert found.__class__ is Match
    assert isinstance(found, Match) and not isinstance(unicode_digits, Match)
    assert issubclass(Match, object) and not issubclass(Match, Pattern)
    assert type(Match) is type and repr(Match) == "<class 're.Match'>"
    assert (Match.__module__, Match.__name__, Match.__qualname__) == ("re", "Match", "Match")
    assert Match[str].__origin__ is Match and Match[bytes].__args__ == (bytes,)
    assert Match.__new__ is object.__new__
    for construct in (lambda: Match(), lambda: Match.__new__(Match)):
        try:
            construct()
        except TypeError as error:
            assert str(error) == "cannot create 're.Match' instances"
        else:
            raise AssertionError("constructed a match without matching")
    try:
        Match.extra = 1
    except TypeError as error:
        assert str(error) == "cannot set 'extra' attribute of immutable type 're.Match'"
    else:
        raise AssertionError("modified re.Match")
    try:
        class SubMatch(Match):
            pass
    except TypeError as error:
        assert str(error) == "type 're.Match' is not an acceptable base type"
    else:
        raise AssertionError("subclassed re.Match")
    print(re.A is re.ASCII, re.U is re.UNICODE)
    print(ascii_digits.pattern, ascii_digits.flags, unicode_digits.flags)
    for text in ("200", "-404", "+12", "1_2", "٢٠٠", "-１２", "200\n"):
        print(text, ascii_digits.fullmatch(text) is not None, unicode_digits.fullmatch(text) is not None)
    print(re.fullmatch(r"(a|ab)", "ab", re.A).group(1))
    print(re.fullmatch(r"[^\d].", "٢é", re.ASCII).group())
    print(re.compile(ascii_digits) is ascii_digits)
    for pattern, flags in ((ascii_digits, re.ASCII), ("a", re.ASCII | re.UNICODE)):
        try:
            re.compile(pattern, flags)
        except ValueError as error:
            print(str(error))

    newline = re.compile("[\r\n]")
    for value in ("application/json", "12", "é\rvalue", "é\nvalue"):
        print(newline.search(value) is not None)
    grouped = re.compile("(?P<letter>β+)(x)?")
    print(grouped.search(string="αββ", pos=1, endpos=3).group(0, "letter", 2))
    print(re.search(grouped, "αβ").group("letter"))
    names = re.match(rb"(?P<z>a)(b)?(?P<a>x)?$", b"a")
    default = []
    assert list(names.re.groupindex.items()) == [("z", 1), ("a", 3)]
    try:
        names.re.groupindex["a"] = 2
    except TypeError:
        pass
    else:
        raise AssertionError("groupindex allowed assignment")
    unnamed = re.compile(r"(x)$")
    unnamed.groupindex["extra"] = 1
    assert unnamed.groupindex == {}
    values = names.groupdict(default=default)
    assert list(values) == ["z", "a"]
    assert values["z"] == b"a" and values["a"] is default
    values["z"] = None
    assert names.groupdict() == {"z": b"a", "a": None}
    assert re.match(r"(x)$", "x").groupdict(default) == {}
    print(re.search(r"\d", "٣2", flags=re.ASCII).group())
    print(re.compile("β$").search("αβx", 1, 2).group())
    print(re.compile("^β").search("αβ", 1) is None)
    print(re.compile("").search("α", 20) is not None, newline.search("α\n", 2, 1) is None)
    for args in ((1,), (b"x",), ("x", None), ("x", 10**100)):
        try:
            newline.search(*args)
        except (TypeError, OverflowError) as error:
            print(str(error))
    for args, kwargs in (((), {}), (("x",), {"string": "y"}), (("x",), {"missing": 1})):
        try:
            newline.search(*args, **kwargs)
        except TypeError as error:
            print(str(error))

    # pos indexes the original string; endpos supplies its logical end.
    text = "xαβz"
    pattern = re.compile("(α|αβ)")
    assert pattern.match(string=text, pos=1, endpos=3).group(1) == "α"
    assert pattern.fullmatch(text, endpos=3, pos=1).group(1) == "αβ"
    assert re.compile("^α").match(text, 1, 3) is None
    assert re.compile("β$").fullmatch(text, 2, 3).group() == "β"
    assert re.compile(b"(.)").match(b"x\xff!z", 1, 2).group(1) == b"\xff"
    assert pattern.match(text, 3, 1) is None and pattern.fullmatch(text, 3, 1) is None
    assert re.compile("x").match(text, -10, 20).group() == "x"

    # CPython can match an empty alternative past endpos. The boxed matcher
    # currently rejects this case rather than losing the original position.
    empty = re.compile("")
    try:
        empty_match = empty.match("aa", 1, 0)
    except NotImplementedError as error:
        assert str(error) == "zero-width matching with pos past endpos is unsupported"
    else:
        assert empty_match is not None and empty_match.span() == (1, 1)
    assert empty.fullmatch("aa", 1, 0) is None

    for spelling, subject, word, pos, end in (
        (r"(Python)\1", "xPythonPythony", "Python", 1, 13),
        (r"(é🙂)\1", "xé🙂é🙂y", "é🙂", 1, 5),
        (b"(\xffA)\\1", b"x\xffA\xffAy", b"\xffA", 1, 5),
    ):
        doubled = re.compile(spelling)
        assert doubled.pattern == spelling
        match = doubled.fullmatch(subject, pos, end)
        assert match is not None and match.group(0, 1) == (word + word, word)
        assert doubled.match(subject, pos + 1, end) is None
        assert doubled.fullmatch(subject, pos, end - 1) is None
    assert re.fullmatch(r"(Python)\1", "PythonPythox") is None

    for spelling, subject, capture, wrong in (
        (r"([a-c]*)\1", "xabcabc!", "abc", "abccba"),
        (rb"([a-c]*)\1", b"xabcabc!", b"abc", b"abccba"),
    ):
        doubled = re.compile(spelling)
        match = doubled.fullmatch(subject, 1, 7)
        assert match.group(1) == capture and match.span(1) == (1, 4)
        assert doubled.match(subject, 1, 7).span() == (1, 7)
        assert doubled.fullmatch(wrong) is None
        assert doubled.match(wrong).span(1) == (0, 0)
    assert re.fullmatch(r"^(a+).\1$", "aaaa") is None
    at_end = re.search(r"(?=(a*)*)\1", "aaa")
    assert at_end.span() == (0, 0) and at_end.span(1) == (3, 3)
    for spelling, subject, expected in (
        (r"((a)?)+\2", "aa", ((1, 1), (0, 1))),
        (rb"(?:(a?)|(b)){0,2}\2\1", b"bb", ((1, 1), (0, 1))),
    ):
        try:
            match = re.fullmatch(spelling, subject)
        except re.PatternError:
            continue
        assert (match.span(1), match.span(2)) == expected
    try:
        high_repeat = re.compile(rb"((?:\200|A)+)\1")
    except re.PatternError:
        pass
    else:
        assert high_repeat.fullmatch(b"\x80\x80").group(1) == b"\x80"
        assert high_repeat.fullmatch(b"\xc2\x80\xc2\x80") is None
    try:
        re.compile(r"(a)\1{2,1}")
    except re.PatternError:
        pass
    else:
        raise AssertionError("accepted a decreasing repetition range")
    mixed = re.fullmatch(rb"(.+)\1", b"\xffA\xffa", re.I)
    assert mixed.group(1) == b"\xffA" and mixed.span() == (0, 4)

    for spelling, name, word in (
        ("(?P<if>ab)(?P=if)", "if", "ab"),
        ("(?P<état>é)(?P=état)", "état", "é"),
        (b"(?P<word>\xff)(?P=word)", "word", b"\xff"),
    ):
        pattern = re.compile(spelling)
        assert pattern.fullmatch(word + word).group(name) == word
        assert pattern.fullmatch(word) is None

    import sys
    if sys.platform.startswith("linux"):
        import _locale
        saved = _locale.setlocale(_locale.LC_CTYPE)
        try:
            _locale.setlocale(_locale.LC_CTYPE, "C")
            locale_ref = re.compile(rb"([a-c]*)\1", re.L | re.I)
            assert locale_ref.fullmatch(b"AbCaBc").group(1) == b"AbC"
            assert locale_ref.fullmatch(b"AbCac") is None
        finally:
            _locale.setlocale(_locale.LC_CTYPE, saved)

    for spelling, flags, word, repeated, wrong in (
        ("(?P<name>ab)(?P=name)", re.I, "Ab", "aB", "ac"),
        (r"(k)\1", re.I, "K", "K", "x"),
        (r"(is)\1", re.I | re.A, "iS", "Is", "ıS"),
        (rb"(is)\1", re.I, b"iS", b"Is", b"ix"),
        (b"(?P<name>a)(?P=name)", re.L, b"a", b"a", b"A"),
    ):
        doubled = re.compile(spelling, flags)
        match = doubled.fullmatch(word + repeated)
        assert match.group(1) == word and match.span(1) == (0, len(word))
        assert doubled.fullmatch(word + wrong) is None

    numbered = re.compile(r"(x)" * 10 + r"(k)(l)\119", re.I)
    prefix = "x" * 10 + "Kl"
    match = numbered.fullmatch(prefix + "k9")
    assert match.group(11, 12) == ("K", "l") and match.span(11) == (10, 11)
    assert numbered.fullmatch(prefix + "l9") is None

    high = re.compile(b"(\xff)(\xfeB)\\2Z")
    assert high.fullmatch(b"\xff\xfeB\xfeBZ").group(2) == b"\xfeB"
    assert high.fullmatch(b"\xff\xfeB\xffBZ") is None

    for letter, other in (("s", "ſ"), ("i", "ı")):
        try:
            doubled = re.compile("(" + letter + r")\1", re.I)
        except re.PatternError:
            continue
        assert doubled.fullmatch(other + other).group(1) == other
        assert doubled.fullmatch(letter + other) is None

    for spelling, message, position in (
        ("é(?P<?foo>x)", "bad character in group name '?foo'", 5),
        ("(?P=<foo>)", "bad character in group name '<foo>'", 4),
        ("(?P<a.b>x)", "bad character in group name 'a.b'", 4),
        (b"(?P<\xb5>x)", r"bad character in group name '\xb5'", 4),
    ):
        try:
            re.compile(spelling)
        except re.PatternError as error:
            assert (error.msg, error.pattern, error.pos) == (message, spelling, position)
        else:
            raise AssertionError("invalid group name accepted")
