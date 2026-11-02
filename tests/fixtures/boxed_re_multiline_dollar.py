import re


def current():
    flags = re.MULTILINE | re.VERBOSE
    lines = re.compile(r"(?P<first>é $) \n ^(?P<second>b $)  # end of line", flags)
    match = lines.search("x\né\nb\ny")
    assert match.groups() == ("é", "b")
    assert match.span() == (2, 5) and match.span("first") == (2, 3)
    assert lines.fullmatch("é\nb")
    assert lines.search("é\nbx", 0, 3).span() == (0, 3)
    assert lines.search("é\nbx") is None
    assert lines.search("é\r\nb") is None

    assert re.fullmatch(rb"(a$)\n(b$)", b"a\nb", flags).groups() == (b"a", b"b")
    assert re.fullmatch(r"\$ [$] $", "$$", flags)
    assert re.search(r"a $", "a\nb", re.VERBOSE) is None
    assert re.search(r"a $", "a\n", re.VERBOSE).span() == (0, 1)
    scanner = re.Scanner([(r"a $ \n b", "pair"), ("a", "single")], flags)
    assert scanner.scan("a\nb!") == (["pair"], "!")
    assert scanner.scan("ab!") == (["single"], "b!")
    print("multiline dollar passed")
