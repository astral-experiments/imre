import re


def current():
    flag = re.I
    print(flag is re.IGNORECASE is re.RegexFlag(2), type(flag) is re.RegexFlag)
    print(isinstance(flag, int), flag.name, flag.value, str(flag), repr(flag))
    for value in (re.I | re.A, re.RegexFlag(3), re.RegexFlag(1 << 70), ~re.I):
        print(type(value) is re.RegexFlag, re.RegexFlag(value.value) is value, value.name, repr(value))
    choices = "20|25|2F|3F"
    pattern = re.compile(f"((?:%(?:{choices}))+)", flag)
    print(pattern.flags, type(pattern.flags) is int, pattern.groups, repr(pattern))
    for value in ("", "/health", "%2f%25", "é%3F%2f東", "x%20a%3fy"):
        print(pattern.split(value), pattern.split(string=value, maxsplit=1))

    for bits in (2, 258):
        print([re.compile("[H-J]", bits).fullmatch(c) is not None for c in "iIİıK"])
        print([re.compile("[^H-J]", bits).fullmatch(c) is not None for c in "iIİıK"])
        print(re.compile("k.", bits).fullmatch("Ké") is not None)
    print(re.compile(b"(A)|(B)", 2).split(b"\xffab"))
    print(re.split("(a)|(b)", "xab", maxsplit=2, flags=re.I))
    for source in ("", "x*", "|x+", "x*?", "(a*)|(b)", r"\b"):
        print(re.compile(source).split("xxaβb"), re.compile(source).split("xxaβb", -1))
    print(re.compile("$").split("a\n"), re.compile("$", re.X | re.M).split("a\nb\n"))
    for source, text in [(r"(\n)??$", "\n"), (rb"(\n)??$", b"x\n")]:
        compiled = re.compile(source)
        print(compiled.findall(text), compiled.split(text))
    long_value = "not a delimiter" * 10
    print(pattern.split(long_value)[0] is long_value)
    try:
        pattern.split("value", 1 << 100)
    except OverflowError as error:
        print(type(error).__name__, str(error))

    assert re.compile(r"((?:\n)??)$").sub("!", "a\n") == "a!!!"
    assert re.compile("|x+").sub(repl="東", string="xxé", count=3) == "東東東é"
    assert re.compile(b"(.)").sub(b"\xff", bytearray(b"ab"), 1) == b"\xffb"
    assert re.compile("(?P<name>x)").sub("$1", "xx") == "$1$1"
    assert re.compile(b"x").sub(memoryview(b"z"), b"xx") == b"zz"
