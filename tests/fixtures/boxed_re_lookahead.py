import re


def current():
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
