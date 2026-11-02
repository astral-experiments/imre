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

    # Repeating a variable expression can accept a different capture on the second copy.
    try:
        variable = re.compile(r"(a|b)\1")
    except re.PatternError:
        pass
    else:
        assert variable.fullmatch("bb").group(1) == "b"
        assert variable.fullmatch("ab") is None

    for spelling, name, word in (
        ("(?P<if>ab)(?P=if)", "if", "ab"),
        ("(?P<état>é)(?P=état)", "état", "é"),
        (b"(?P<word>\xff)(?P=word)", "word", b"\xff"),
    ):
        pattern = re.compile(spelling)
        assert pattern.fullmatch(word + word).group(name) == word
        assert pattern.fullmatch(word) is None

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
