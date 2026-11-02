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
