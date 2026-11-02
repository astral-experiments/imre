import re
import sys


def current():
    number = re.compile(r"""
        (?P<sign>[-+]?)
        (?=\d|\.\d)  # A digit is required somewhere before the exponent.
        (?P<whole>\d*) (?: \.(?P<decimal>\d*) )?
        (?: E(?P<exp>\d+) )?
    """, re.VERBOSE | re.IGNORECASE)
    assert number.groups == 4
    match = number.fullmatch("-.٣e2")
    assert match.groups() == ("-", "", "٣", "2")
    assert match.groupdict() == {"sign": "-", "whole": "", "decimal": "٣", "exp": "2"}
    assert match.span(2) == (1, 1) and match.span("decimal") == (2, 3)
    assert number.fullmatch("7.").groups() == ("", "7", "", None)
    for text in ("", ".", "+e7"):
        assert number.fullmatch(text) is None

    spec = re.compile(r"""
        (?P<zeropad>0(?=[0-9]))? (?P<width>[0-9]+)?
        (?: \. (?=[,_0-9]) (?P<precision>[0-9]+)? [,_]? )? f
    """, re.VERBOSE)
    assert spec.fullmatch("0f").groups() == (None, "0", None)
    assert spec.fullmatch("01.2f").groups() == ("0", "1", "2")
    assert spec.fullmatch(".,f").groups() == (None, None, None)
    assert spec.fullmatch(".f") is None

    ahead = re.compile(rb"(?P<byte>\xff)(?=x)", re.I)
    assert ahead.match(b"\xffX").span() == (0, 1)
    assert ahead.search(b"x\xffx", 1, 2) is None
    assert ahead.search(b"x\xffx", 1, 3).group("byte") == b"\xff"
    assert ahead.fullmatch(b"\xffx") is None

    empty = re.compile(r"(?=é)|é")
    assert [m.span() for m in empty.finditer("xé")] == [(1, 1), (1, 2)]
    assert empty.split("xé") == ["x", "", ""]
    line_end = re.compile(r"(?=a$)")
    assert line_end.search("a\n").span() == (0, 0)
    assert line_end.search("a\nb") is None

    names = re.compile(r"(?P<_imre_lookahead_0>a)(?=b)(?P<end>b)")
    match = names.fullmatch("ab")
    assert names.groups == 2 and match.groups() == ("a", "b")
    assert match.groupdict() == {"_imre_lookahead_0": "a", "end": "b"}
    assert match.span(2) == (1, 2)

    for source, groups in ((r"(?=(a))a", ("a",)), (r"(?=a+)a", ())):
        try:
            pattern = re.compile(source)
        except re.PatternError:
            assert sys.implementation.name == "imre"
        else:
            assert pattern.fullmatch("a").groups() == groups

    comment = re.compile("(?x:a # (?=ignored\n b)")
    assert comment.fullmatch("ab") and comment.groups == 0
    layout = re.compile(r"(?x)a (?-x: # ) b")
    assert layout.flags & re.X
    assert layout.fullmatch("a # b") and layout.fullmatch("ab") is None

    try:
        scoped = re.compile(r"(?i:(?=a).)")
    except re.PatternError:
        assert sys.implementation.name == "imre"
    else:
        assert scoped.fullmatch("A") and scoped.fullmatch("b") is None
    print("positive lookahead passed")
