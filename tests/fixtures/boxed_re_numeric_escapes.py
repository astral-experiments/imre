import re


def current():
    assert re.fullmatch(r"\018\U0001F40D", "\x018🐍")
    assert re.fullmatch(br"\3778", b"\xff8")
    assert re.fullmatch(r"[\18]+", "\x018") and not re.fullmatch(r"[\18]", "1")
    assert re.fullmatch(r"\\x1", r"\x1")
    assert re.fullmatch(r"(?x)\x61 # \xZ", "a")

    text = "\udcffA"
    assert re.search(r"[\101]", text).span() == (1, 2)
    assert re.match(r"[\101]", text) is None
    assert re.match(r"[^\101]", text).span() == (0, 1)
    assert re.search(r"\377", "\udcff\xff").span() == (1, 2)

    eof = "bad escape (end of pattern)"
    for pattern, message, pos, line, column in (
        (r"\x{41}", r"incomplete escape \x", 0, 1, 1),
        ("🌈\né" + r"[\x1z]", r"incomplete escape \x1", 4, 2, 3),
        (b"\xff\n" + br"\U00012345", r"bad escape \U", 2, 2, 1),
        (r"[\U00110000]", r"bad escape \U00110000", 1, 1, 2),
        (r"\567", r"octal escape value \567 outside of range 0-0o377", 0, 1, 1),
        (br"[\911]", r"bad escape \9", 1, 1, 2),
        (r"\911" + "\\", "invalid group reference 91", 1, 1, 2),
        (r"\x1" + "\\", eof, 3, 1, 4),
        (r"\x1z" + "\\", r"incomplete escape \x1", 0, 1, 1),
        (r"\567" + "\\", eof, 4, 1, 5),
        (r"a**\xZ", "multiple repeat", 2, 1, 3),
        (r"(?x)# \x" + "\n" + r"\x{41}", r"incomplete escape \x", 9, 2, 1),
    ):
        try:
            re.compile(pattern)
        except re.PatternError as error:
            assert error.pattern == pattern
            assert (error.msg, error.pos, error.lineno, error.colno) == (message, pos, line, column)
        else:
            raise AssertionError(pattern)
