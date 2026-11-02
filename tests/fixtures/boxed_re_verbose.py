import re


def current():
    for invalid, message, position in (
        ("(?i)a**", "multiple repeat", 6),
        (rb"x{1,2}+?", "multiple repeat", 7),
        ("^*", "nothing to repeat", 1),
        ("(^*)**", "nothing to repeat", 2),
        (rb"\b*", "nothing to repeat", 2),
        ("(?x)a{1,2} #text\n +", "multiple repeat", 18),
    ):
        try:
            re.compile(invalid)
        except re.PatternError as error:
            assert (error.msg, error.pos) == (message, position)
        else:
            raise AssertionError(invalid)
    assert re.fullmatch(r"(?:a*)*", "aa")
    assert re.fullmatch(rb"[\^*+]+", b"^*+")
    assert re.fullmatch(r"a+?", "aaa")
    assert re.fullmatch(rb"[\b]*", b"\x08")
    assert re.fullmatch(rb"(?:\b)*a", b"a")
    assert re.fullmatch(r"(a*)+a", "aa")
    assert re.compile(r"a*+a")
    assert re.compile(r"\<*a++")
    assert re.compile(r"x\b{start}*a++")
    assert re.compile("(?mx)# tail\\\n a**\n a")

    for flags in (0, re.X):
        assert re.fullmatch(r"([\1])\078", "\x01\x078", flags).group(1) == "\x01"
        assert re.fullmatch(rb"[\200-\377]", b"\xff", flags)
        assert re.fullmatch(rb"[\200-\377]", b"\x7f", flags) is None
        assert re.fullmatch(r"(?=\141)a", "a", flags)
    for invalid in (r"\1", r"[\8]", rb"[\400]", rb"\777", "(?x)# [\n" + r"\1"):
        try:
            re.compile(invalid)
        except re.PatternError:
            pass
        else:
            raise AssertionError(invalid)

    tokens = re.compile(
        r"""
        ([\w#-]*)'  # encoding, possibly empty
        [\w#-]*'    # language
        ([\w#%']+)  # encoded payload
        """,
        re.ASCII | re.VERBOSE,
    )
    print(re.X is re.VERBOSE, tokens.flags, tokens.groups)
    for raw in ("utf-8'en'caf%C3%A9", "''plain", "#'fr'#x", "utfé'en'x", "utf-8'en'"):
        match = tokens.match(raw)
        print(None if match is None else match.groups())
    pattern = re.compile(r"(?P<optional>\w+)? \# ([ #]+) $  # tail", re.X)
    fallback = []
    result = pattern.match("# #\n")
    print(pattern.groups, pattern.flags, result.group(), result.groups())
    print(result.groups(default=fallback)[0] is fallback, result.groups("absent"))
    print(pattern.fullmatch("# #\n") is None, pattern.fullmatch("# #").groups())
    print(pattern.match("é# #").group("optional"))
    print(re.match(r"a \s # ignored", "a\x1c", re.X).groups())
    for flags in (0, re.VERBOSE, re.ASCII | re.IGNORECASE):
        guarded = re.compile(r"(?:(?!x|:)[^!])+", flags)
        assert guarded.search("aXbc").group() == ("a" if int(flags & re.I) != 0 else "aXbc")
        assert guarded.fullmatch("abc").group() == "abc"
    for flags in (0, re.VERBOSE):
        magic = re.compile(b"([*?[])", flags)
        assert magic.search(b"name[1]").group(1) == b"["
        assert magic.search(b"name]") is None
        assert re.fullmatch(r"{\w+}", "{width}", flags | re.I).group() == "{width}"
        assert re.fullmatch(rb"{\w+}", b"{width}", flags).group() == b"{width}"
        assert re.fullmatch(rb"\xff{word}", b"\xff{word}", flags)
        assert re.fullmatch(r"\xff{word}", "ÿ{word}", flags)
        assert re.fullmatch(r"{\w+}", "{\u0345}", flags | re.I) is None
        assert re.fullmatch(r"{\W}", "{\u0345}", flags | re.I)
        for invalid in (br"\u0061{word}", r"a{foo}*{2}"):
            try:
                re.compile(invalid, flags)
            except re.PatternError:
                pass
            else:
                raise AssertionError("invalid pattern compiled")
        assert re.fullmatch(r"[{}]\{a{2}}", "{{aa}", flags)
    assert re.fullmatch("(?x:a # {ignored}\n)b", "ab")
    inline = re.compile("(?x) # one\n (?x) a (?-x: [#]) (?x: \u0085 )")
    assert inline.flags == re.X | re.U
    assert inline.fullmatch("a #\u0085")
    assert inline.fullmatch("a #") is None
    scoped = re.compile(b" a(?x: [ #] b) c")
    assert scoped.flags == 0
    assert scoped.fullmatch(b" a#b c")
    assert scoped.fullmatch(b"a#bc") is None
    octets = re.compile(rb"[\x00-\x19\",;\\\x7f-\xff]", re.A)
    print(type(octets.pattern) is bytes, octets.flags, octets.groups)
    print([n for n in range(256) if octets.fullmatch(bytes([n])) is not None])
    raw = bytes([255])
    captured = re.compile(b"(" + raw + b")?(x)").fullmatch(bytearray(raw + b"x"))
    print(captured.group() == raw + b"x", captured.groups() == (raw, b"x"))
    print(re.match(rb"(y)?(x)", b"x").groups(default=fallback)[0] is fallback)
    print(re.compile(rb"(\xff)").findall(raw + raw) == [raw, raw])
    search = re.compile(rb"(\xff)(x)?")
    print(search.search(b"a" + raw + b"x", pos=1, endpos=2).groups())
    print(re.search(r" (é) \# ", "前é#", re.X).groups())
    print(re.compile(rb"(a+) | (b)", re.X).fullmatch(b"b").groups())
    print(re.compile(r"a{,2}").match("aaa").group())
    print(re.compile(rb"(ab){,2}?x").fullmatch(b"ababx").groups())
    print(re.compile(r"[a{,2}]{,}").fullmatch("{,2}").group())
    print([c for c in "0{,2}]" if re.fullmatch(r"[]{,2}]", c)])
    print([c for c in "\b]x" if re.fullmatch(r"[]\b]", c)])
    for pattern, subject in ((octets, "x"), (tokens, b"x")):
        try:
            pattern.match(subject)
        except TypeError as error:
            print(str(error))
