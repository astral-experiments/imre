import gc
import re
from array import array


def current():
    pattern = re.compile("(?P<word>x*)|é")
    assert re.sub(pattern, "_", "éx", count=1) == "_éx"
    assert re.subn(pattern, "_", "éx") == ("____", 4)
    assert pattern.subn("_", "éx", 1) == ("_éx", 1)
    assert re.compile(b"x").subn(b"x", bytearray(b"x-x")) == (b"x-x", 2)
    expand = re.search(r"(?P<word>é)(x)?", "!é").expand
    assert expand(template=r"\g<0>/\g<word>/\2") == "é/é/"
    assert expand(br"\2") == b""
    source = bytearray(b"x-y")
    match = re.search(br"(?P<letter>y)", source)
    source[-1] = ord("z")
    assert match.expand(bytearray(br"<\1\g<letter>>")) == b"<zz>"
    for template, error, message in (
        (r"\g<absent>", IndexError, "unknown group name 'absent'"),
        (r"\3", re.PatternError, "invalid group reference 3 at position 1"),
        (br"\1", TypeError, "sequence item 1: expected a bytes-like object, str found"),
    ):
        try:
            expand(template)
        except error as exc:
            assert str(exc) == message
        else:
            raise AssertionError("invalid expansion accepted")
    pairs = re.compile(r"(a)?b")
    assert pairs.subn(r"<\1>", "ab-b") == ("<a>-<>", 2)
    assert pairs.subn(r"<\1>", "no match") == ("no match", 0)
    for kwargs, args, message in (
        ({"unknown": 0}, (), "subn() missing required argument 'repl' (pos 1)"),
        ({"countt": 1}, ("_", "ab"), "subn() got an unexpected keyword argument 'countt'. Did you mean 'count'?"),
    ):
        try:
            pairs.subn(*args, **kwargs)
        except TypeError as error:
            assert str(error) == message
        else:
            raise AssertionError("invalid subn arguments accepted")
    compile_pattern = re.compile
    try:
        re.compile = None
        assert re.sub(b"a", lambda m: m.group() + b"!", b"aA", flags=re.I) == b"a!A!"
    finally:
        re.compile = compile_pattern
    subject = "éx"
    seen = []

    def replace(match):
        gc.collect()
        assert match.string is subject and match.re is pattern
        seen.append((match.span(), match.group("word")))
        return None if match.group() == "é" else r"\1"

    sub = pattern.sub
    assert sub(repl=replace, string=subject) == r"\1\1\1"
    assert seen == [((0, 0), ""), ((0, 1), None), ((1, 2), "x"), ((2, 2), "")]
    seen.clear()
    assert pattern.subn(repl=replace, string=subject) == (r"\1\1\1", 4)
    assert seen == [((0, 0), ""), ((0, 1), None), ((1, 2), "x"), ((2, 2), "")]
    untouched, count = pattern.subn(replace, subject, count=-1)
    assert untouched is subject and count == 0 and len(seen) == 4
    seen.clear()
    assert sub(replace, subject, count=1) == r"\1éx"
    assert sub(replace, subject, count=-1) is subject and len(seen) == 1

    def bad(match):
        seen.append(match.start())
        if match.start() == 2:
            raise ValueError("last callback")
        return 123

    dots = re.compile(".")
    seen.clear()
    try:
        dots.sub(bad, "abc")
    except ValueError as error:
        assert error.args == ("last callback",) and seen == [0, 1, 2]
    else:
        raise AssertionError("callback exception lost")
    for regex, value, item, expected in (
        (re.compile("a"), "ba", 1, "sequence item 1: expected str instance, int found"),
        (re.compile(b"a"), b"ba", "", "sequence item 1: expected a bytes-like object, str found"),
    ):
        try:
            regex.sub(lambda m: item, value)
        except TypeError as error:
            assert str(error) == expected
        else:
            raise AssertionError("invalid replacement accepted")

    source = bytearray(b"za-a-a!")
    result = bytearray(b"x")
    retained = []

    def mutate(match):
        retained.append(match)
        if match.start() == 1:
            try:
                source.extend(b"z")
            except BufferError:
                pass
            else:
                raise AssertionError("match input was resizable")
            source[0] = 90
            source[3] = 98
            return result
        result[0] = 89
        return memoryview(b"y")

    assert re.compile(b"a").sub(mutate, source) == b"zY-b-y!"
    assert [m.start() for m in retained] == [1, 5]
    source.extend(b"?")
    source[1] = 99
    assert retained[0].string is source and retained[0].group() == b"c"

    # The punctuation escape in _strptime.TimeRE.pattern prefixes the captured character.
    assert re.sub(r"([\\.^$*+?\(\){}\[\]|])", r"\\\1", "[é].") == r"\[é\]\."
    groups = re.compile(r"(?P<one>a)?(b)$")
    assert groups.sub(r"<\g<one>/\2/\g<0>>", "b\n") == "</b/b>\n"
    assert re.compile("(|é)").sub(r"[\1]", "é", 2) == "[][é]"
    assert re.compile(b"(.)").sub(bytearray(rb"\377\1"), b"\x81") == b"\xff\x81"
    for count in (-1, 0):
        try:
            groups.sub(r"é\3", "", count)
        except re.PatternError as exc:
            assert (exc.msg, exc.pattern, exc.pos) == ("invalid group reference 3", r"é\3", 2)
        else:
            raise AssertionError("missing group accepted before matching")
    try:
        groups.sub(r"\g<absent>", "b")
    except IndexError as exc:
        assert exc.args == ("unknown group name 'absent'",)
    else:
        raise AssertionError("unknown named group accepted")
    for invalid in ("\\q\\", "\\2\\", "\\400\\", "\\g<x>\\"):
        try:
            groups.sub(invalid, "")
        except re.PatternError as exc:
            assert exc.msg == "bad escape (end of pattern)" and exc.pos == len(invalid) - 1
        else:
            raise AssertionError("incomplete trailing escape ignored")
    try:
        re.compile(b"(x)").sub(memoryview(array('B', [92, 49])), b"y")
    except ValueError as exc:
        assert str(exc) == "cannot hash writable memoryview object"
    else:
        raise AssertionError("unhashable writable memoryview template accepted")

    subject = "\ud800\ue000Xé\udfff!"
    assert re.sub("X", "_", subject) == "\ud800\ue000_é\udfff!"
    assert re.sub("X", "\udcfe", subject) == "\ud800\ue000\udcfeé\udfff!"
    assert re.subn("(|.)", r"<\1>", "\udfff") == ("<><\udfff><>", 3)
    assert re.compile("XX").sub("_", subject) is subject
    captures = re.compile(r"(?P<ch>.)(X)?")
    assert captures.subn(r"\g<ch>/<\2>", subject, 2) == ("\ud800/<>\ue000/<X>é\udfff!", 2)
    seen = []

    def keep_first(match):
        assert match.string is subject and match.re is captures
        assert (match.lastindex, match.lastgroup) == ((2, None) if match.group(2) else (1, "ch"))
        seen.append((match.span(), match.groups()))
        return match.group(1)

    assert captures.sub(keep_first, subject) == "\ud800\ue000é\udfff!"
    assert seen == [
        ((0, 1), ("\ud800", None)), ((1, 3), ("\ue000", "X")), ((3, 4), ("é", None)),
        ((4, 5), ("\udfff", None)), ((5, 6), ("!", None)),
    ]

    edge = re.compile("[\ud7ff-\ud800\udfff-\ue000]+")
    text = "\ud7ff\ud800\ud801\U000f0000\udfff\ue000"
    assert edge.subn(lambda m: "<" + m[0] + ">", text) == (
        "<\ud7ff\ud800>\ud801\U000f0000<\udfff\ue000>", 2,
    )
    assert edge.subn("-", "\ud7ff\U000f0000\ue000") == ("-\U000f0000-", 2)
    assert re.compile("[^\ud800-\udfff]+").sub("-", text) == "-\ud800\ud801-\udfff-"
    above = re.compile("[\udfff-\U0010ffff]+")
    assert above.findall("\udc80\udfff\U000f04ff\udcff") == ["\udfff\U000f04ff"]

    escaped = re.compile(r"[\ud800-\U0000dfff]+")
    assert escaped.pattern == r"[\ud800-\U0000dfff]+"
    assert escaped.subn("-", "\U000f04ff\udcff\ud800") == ("\U000f04ff-", 1)
    scalar = re.compile("[^\u0000-\ud7ff\ue000-\U0010ffff]")
    assert scalar.search("\ud7ff\ue000\U0010ffff") is None
    assert scalar.search("\U000f04ff\udcff").span() == (1, 2)
    try:
        re.compile(r"[\udfff-\ud800]")
    except re.PatternError as exc:
        assert (exc.msg, exc.pos) == (r"bad character range \u-\u", 9)
    else:
        raise AssertionError("descending Unicode escape range accepted")

    outside = re.compile(r"[^a\ud7ff\ue000]", re.I)
    assert outside.findall("A\ud7fe\ud7ff\ud800\ue000\ue001\U000f04ff") == [
        "\ud7fe", "\ud800", "\ue001", "\U000f04ff",
    ]
